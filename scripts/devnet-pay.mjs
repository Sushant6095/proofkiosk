#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

import { address, createKeyPairSignerFromBytes } from "@solana/kit";
import { createSolanaPayClient } from "@solana/pay";

function required(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`missing ${name}; run and source scripts/devnet-setup.sh output first`);
  return value;
}

function keypairPath(value) {
  return value.startsWith("~/") ? join(homedir(), value.slice(2)) : value;
}

const keypairBytes = JSON.parse(
  await readFile(keypairPath(required("PROOFKIOSK_CUSTOMER_KEYPAIR")), "utf8"),
);
if (
  !Array.isArray(keypairBytes) ||
  keypairBytes.length !== 64 ||
  keypairBytes.some((byte) => !Number.isInteger(byte) || byte < 0 || byte > 255)
) {
  throw new Error("PROOFKIOSK_CUSTOMER_KEYPAIR must point to a 64-byte Solana CLI JSON keypair");
}

const amountText = required("PROOFKIOSK_AMOUNT");
if (!/^(?:0|[1-9]\d*)(?:\.\d{1,6})?$/.test(amountText)) {
  throw new Error("PROOFKIOSK_AMOUNT must be a positive plain decimal with at most 6 fraction digits");
}
const amount = Number(amountText);
if (!Number.isFinite(amount) || amount <= 0) {
  throw new Error("PROOFKIOSK_AMOUNT must be greater than zero");
}
const maxAmount = Number(process.env.PROOFKIOSK_MAX_AMOUNT ?? "10");
if (!Number.isFinite(maxAmount) || maxAmount <= 0 || amount > maxAmount) {
  throw new Error(`PROOFKIOSK_AMOUNT must not exceed the test cap (${maxAmount})`);
}

const rpcUrl = required("PROOFKIOSK_RPC_URL");
const parsedRpcUrl = new URL(rpcUrl);
const isLocal = ["127.0.0.1", "localhost", "::1"].includes(parsedRpcUrl.hostname);
const isPublicDevnet = parsedRpcUrl.hostname === "api.devnet.solana.com";
if (!isLocal && !isPublicDevnet && process.env.PROOFKIOSK_ALLOW_CUSTOM_DEVNET_RPC !== "1") {
  throw new Error(
    "refusing a non-local, non-public-Devnet RPC; set PROOFKIOSK_ALLOW_CUSTOM_DEVNET_RPC=1 only for a verified custom Devnet endpoint",
  );
}

const payer = await createKeyPairSignerFromBytes(Uint8Array.from(keypairBytes));
const referenceText = required("PROOFKIOSK_REFERENCE");
const itemId = required("PROOFKIOSK_ITEM");
const fields = {
  recipient: address(required("PROOFKIOSK_MERCHANT")),
  amount,
  splToken: address(required("PROOFKIOSK_MINT")),
  reference: address(referenceText),
  memo: JSON.stringify({ v: 1, tag: "PKPAY1", ref: referenceText, item: itemId }),
};
const client = createSolanaPayClient({
  rpcUrl,
  payer,
});

const instructions = await client.pay.createTransfer(fields);
const result = await client.sendTransaction(instructions);
const signature = result.context.signature;
console.log(`submitted ${signature}`);

const timeout = Number(process.env.PROOFKIOSK_FINALIZE_TIMEOUT_MS ?? 120_000);
if (!Number.isSafeInteger(timeout) || timeout < 1_000 || timeout > 600_000) {
  throw new Error("PROOFKIOSK_FINALIZE_TIMEOUT_MS must be an integer from 1000 to 600000");
}
const deadline = Date.now() + timeout;
let lastRpcError;
for (;;) {
  if (Date.now() >= deadline) {
    const detail = lastRpcError ? `; last RPC error: ${lastRpcError}` : "";
    throw new Error(`finalization timeout: ${signature}${detail}`);
  }
  try {
    const requestTimeout = Math.min(10_000, Math.max(1, deadline - Date.now()));
    const { value: [status] } = await client.rpc
      .getSignatureStatuses([signature], { searchTransactionHistory: true })
      .send({ abortSignal: AbortSignal.timeout(requestTimeout) });
    if (status?.err) {
      const detail = JSON.stringify(status.err, (_, value) =>
        typeof value === "bigint" ? value.toString() : value,
      );
      throw new Error(`transaction failed: ${detail}`);
    }
    if (status?.confirmationStatus === "finalized") break;
    lastRpcError = undefined;
  } catch (error) {
    if (error instanceof Error && error.message.startsWith("transaction failed:")) throw error;
    lastRpcError = error instanceof Error ? error.message : String(error);
  }
  await new Promise((resolve) => setTimeout(resolve, 750));
}

await client.pay.validateTransfer(signature, fields, { commitment: "finalized" });
console.log(`finalized and validated ${signature}`);
