use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use zeroclaw_api::tool::Tool;
use zeroclaw_plugins::PluginPermission;
use zeroclaw_plugins::component::PluginLimits;
use zeroclaw_plugins::wasm_tool::WasmTool;

const MERCHANT: &str = "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T";
const MINT: &str = "So11111111111111111111111111111111111111112";
const DEVICE: &str = "Stake11111111111111111111111111111111111111";
const AUTHORITY: &str = "Vote111111111111111111111111111111111111111";
const CUSTOMER: &str = "CgembYUd2JeXgJ2dLe6uETLKxkDdbvDcrNabAE33Jf2t";
const MERCHANT_TOKEN_ACCOUNT: &str = "8qbHbw2BbbTHBW1sbeqakYXVKRQM8Ne7pLK7m6CVfeR";
const SOURCE_TOKEN_ACCOUNT: &str = "CktRuQ2mttgRGkXJtyksdKHjUdc2C4TgDzyB98oEzy8";
const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
const RECENT_BLOCKHASHES_SYSVAR: &str = "SysvarRecentB1ockHashes11111111111111111111";
const RENT_SYSVAR: &str = "SysvarRent111111111111111111111111111111111";
const COMPUTE_BUDGET_PROGRAM: &str = "ComputeBudget111111111111111111111111111111";
const MEMO_PROGRAM: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const PAYMENT_SIGNATURE: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const NONCE_INIT_SIGNATURE: &str = "nonce-init-signature";
const NONCE_CONTEXT_SLOT: u64 = 4_242;
const WASM_EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);
// Current + Initialized, AUTHORITY, a deterministic durable nonce, and a fee calculator.
const NONCE_DATA_BASE64: &str = "AQAAAAEAAAAHYUgdNXR0u3xNdiTr072z2DVec9EQQ/wNo1OAAAAAABERERERERERERERERERERERERERERERERERERERERERiBMAAAAAAAA=";

struct RpcStep {
    method: &'static str,
    result: Value,
}

struct RpcMockServer {
    url: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<Vec<Value>>>,
}

impl RpcMockServer {
    async fn start(steps: Vec<RpcStep>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind local JSON-RPC server")?;
        let address = listener.local_addr().context("read mock RPC address")?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(run_rpc_server(listener, steps.into(), shutdown_rx));
        Ok(Self {
            url: format!("http://{address}"),
            shutdown: Some(shutdown_tx),
            task,
        })
    }

    async fn finish(mut self) -> Result<Vec<Value>> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let joined = tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .context("local JSON-RPC server did not shut down")?;
        joined.context("local JSON-RPC server task failed")?
    }
}

async fn run_rpc_server(
    listener: TcpListener,
    mut steps: VecDeque<RpcStep>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<Vec<Value>> {
    let mut requests = Vec::new();
    while !steps.is_empty() {
        let accepted = tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => accepted.context("accept mock RPC connection")?,
        };
        let (mut stream, _) = accepted;
        let request = read_http_json(&mut stream).await?;
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .context("mock RPC request has no method")?;
        let step = steps.pop_front().context("unexpected extra RPC request")?;
        ensure!(
            method == step.method,
            "expected RPC method {}, received {method}",
            step.method
        );
        ensure!(request["jsonrpc"] == "2.0", "request is not JSON-RPC 2.0");
        ensure!(request["id"] == 1, "request id is not 1");
        requests.push(request);

        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": step.result,
        })
        .to_string();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        );
        stream
            .write_all(headers.as_bytes())
            .await
            .context("write mock RPC headers")?;
        stream
            .write_all(response.as_bytes())
            .await
            .context("write mock RPC body")?;
        stream.shutdown().await.context("close mock RPC response")?;
    }
    ensure!(
        steps.is_empty(),
        "mock RPC server stopped before serving: {}",
        steps
            .iter()
            .map(|step| step.method)
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(requests)
}

async fn read_http_json(stream: &mut TcpStream) -> Result<Value> {
    const MAX_REQUEST_BYTES: usize = 64 * 1024;
    let mut bytes = Vec::new();
    let (header_end, content_length) = loop {
        ensure!(
            bytes.len() < MAX_REQUEST_BYTES,
            "mock RPC request exceeded {MAX_REQUEST_BYTES} bytes"
        );
        let mut chunk = [0u8; 4096];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .context("timed out reading mock RPC request")??;
        ensure!(read != 0, "mock RPC client closed before sending a body");
        bytes.extend_from_slice(&chunk[..read]);

        if let Some(header_end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let header_end = header_end + 4;
            let headers = std::str::from_utf8(&bytes[..header_end])
                .context("mock RPC headers are not UTF-8")?;
            ensure!(
                headers.starts_with("POST "),
                "mock RPC received a non-POST request"
            );
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>())
                .transpose()
                .context("invalid Content-Length")?;
            let chunked = headers.lines().any(|line| {
                line.split_once(':').is_some_and(|(name, value)| {
                    name.eq_ignore_ascii_case("transfer-encoding")
                        && value
                            .split(',')
                            .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
                })
            });
            ensure!(
                content_length.is_some() || chunked,
                "mock RPC request has neither Content-Length nor chunked framing"
            );
            break (header_end, content_length);
        }
    };

    let body = if let Some(content_length) = content_length {
        ensure!(
            header_end.saturating_add(content_length) <= MAX_REQUEST_BYTES,
            "mock RPC body exceeded {MAX_REQUEST_BYTES} bytes"
        );
        while bytes.len() < header_end + content_length {
            read_http_bytes(stream, &mut bytes, MAX_REQUEST_BYTES).await?;
        }
        bytes[header_end..header_end + content_length].to_vec()
    } else {
        loop {
            if let Some(body) = decode_chunked_body(&bytes[header_end..])? {
                break body;
            }
            read_http_bytes(stream, &mut bytes, MAX_REQUEST_BYTES).await?;
        }
    };
    serde_json::from_slice(&body).context("mock RPC body is not JSON")
}

async fn read_http_bytes(
    stream: &mut TcpStream,
    bytes: &mut Vec<u8>,
    max_request_bytes: usize,
) -> Result<()> {
    ensure!(
        bytes.len() < max_request_bytes,
        "mock RPC request exceeded {max_request_bytes} bytes"
    );
    let mut chunk = [0u8; 4096];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
        .await
        .context("timed out reading mock RPC body")??;
    ensure!(read != 0, "mock RPC client closed during its body");
    bytes.extend_from_slice(&chunk[..read]);
    ensure!(
        bytes.len() <= max_request_bytes,
        "mock RPC request exceeded {max_request_bytes} bytes"
    );
    Ok(())
}

fn decode_chunked_body(encoded: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut cursor = 0;
    let mut body = Vec::new();
    loop {
        let Some(line_length) = encoded[cursor..].windows(2).position(|w| w == b"\r\n") else {
            return Ok(None);
        };
        let line_end = cursor + line_length;
        let size_text =
            std::str::from_utf8(&encoded[cursor..line_end]).context("chunk size is not UTF-8")?;
        let size_text = size_text
            .split_once(';')
            .map_or(size_text, |(size, _)| size);
        let size = usize::from_str_radix(size_text.trim(), 16).context("invalid chunk size")?;
        cursor = line_end + 2;
        if size == 0 {
            if encoded.len() < cursor + 2 {
                return Ok(None);
            }
            ensure!(
                &encoded[cursor..cursor + 2] == b"\r\n",
                "mock RPC chunked request has trailers"
            );
            return Ok(Some(body));
        }
        if encoded.len() < cursor.saturating_add(size).saturating_add(2) {
            return Ok(None);
        }
        body.extend_from_slice(&encoded[cursor..cursor + size]);
        cursor += size;
        ensure!(
            &encoded[cursor..cursor + 2] == b"\r\n",
            "mock RPC chunk lacks its terminator"
        );
        cursor += 2;
    }
}

fn decimal_base_units(amount: &str, decimals: u8) -> Result<u64> {
    let (whole, fraction) = amount.split_once('.').unwrap_or((amount, ""));
    ensure!(
        fraction.len() <= usize::from(decimals),
        "charge amount has too many decimal places"
    );
    let scale = 10u64
        .checked_pow(u32::from(decimals))
        .context("token decimal scale overflow")?;
    let whole = whole
        .parse::<u64>()
        .context("invalid charge whole amount")?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u64>()
            .context("invalid charge fractional amount")?
            .checked_mul(
                10u64
                    .checked_pow(u32::from(decimals) - fraction.len() as u32)
                    .context("token fraction scale overflow")?,
            )
            .context("charge fractional amount overflow")?
    };
    whole
        .checked_mul(scale)
        .and_then(|units| units.checked_add(fraction))
        .context("charge amount overflows base units")
}

fn watch_steps(
    merchant: &str,
    mint: &str,
    decimals: u8,
    amount: &str,
    reference: &str,
    item_id: &str,
) -> Result<Vec<RpcStep>> {
    let amount = decimal_base_units(amount, decimals)?.to_string();
    let memo = json!({
        "v": 1,
        "tag": "PKPAY1",
        "ref": reference,
        "item": item_id,
    })
    .to_string();
    let block_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("host clock is before Unix epoch")?
        .as_secs();
    let signatures = json!([{
        "signature": PAYMENT_SIGNATURE,
        "slot": 4_000,
        "err": null,
        "blockTime": block_time,
        "memo": format!("[{}] {memo}", memo.len()),
        "confirmationStatus": "finalized",
    }]);
    let transaction = json!({
        "slot": 4_000,
        "blockTime": block_time,
        "meta": {
            "err": null,
            "preTokenBalances": [{
                "accountIndex": 1,
                "mint": mint,
                "owner": merchant,
                "uiTokenAmount": { "amount": "0", "decimals": decimals }
            }],
            "postTokenBalances": [{
                "accountIndex": 1,
                "mint": mint,
                "owner": merchant,
                "uiTokenAmount": { "amount": amount, "decimals": decimals }
            }]
        },
        "transaction": {
            "message": {
                "accountKeys": [
                    { "pubkey": CUSTOMER, "signer": true, "writable": true },
                    { "pubkey": MERCHANT_TOKEN_ACCOUNT, "signer": false, "writable": true },
                    { "pubkey": SOURCE_TOKEN_ACCOUNT, "signer": false, "writable": true },
                    { "pubkey": mint, "signer": false, "writable": false },
                    { "pubkey": reference, "signer": false, "writable": false },
                    { "pubkey": COMPUTE_BUDGET_PROGRAM, "signer": false, "writable": false },
                    { "pubkey": MEMO_PROGRAM, "signer": false, "writable": false },
                    { "pubkey": TOKEN_PROGRAM, "signer": false, "writable": false }
                ],
                "instructions": [
                    { "programId": COMPUTE_BUDGET_PROGRAM, "accounts": [], "data": "" },
                    { "program": "spl-memo", "programId": MEMO_PROGRAM, "parsed": memo },
                    {
                        "program": "spl-token",
                        "programId": TOKEN_PROGRAM,
                        "parsed": {
                            "type": "transferChecked",
                            "info": {
                                "source": SOURCE_TOKEN_ACCOUNT,
                                "destination": MERCHANT_TOKEN_ACCOUNT,
                                "mint": mint,
                                "multisigAuthority": CUSTOMER,
                                "signers": [reference],
                                "tokenAmount": { "amount": amount, "decimals": decimals }
                            }
                        }
                    }
                ]
            }
        }
    });
    Ok(vec![
        RpcStep {
            method: "getSignaturesForAddress",
            result: signatures,
        },
        RpcStep {
            method: "getTransaction",
            result: transaction,
        },
    ])
}

fn attest_steps() -> Vec<RpcStep> {
    let account_info = json!({
        "context": { "slot": NONCE_CONTEXT_SLOT },
        "value": {
            "data": [NONCE_DATA_BASE64, "base64"],
            "executable": false,
            "owner": SYSTEM_PROGRAM,
        }
    });
    let signatures = json!([{
        "signature": NONCE_INIT_SIGNATURE,
        "slot": NONCE_CONTEXT_SLOT,
        "memo": null,
        "err": null,
    }]);
    let initialization = json!({
        "meta": { "err": null },
        "transaction": {
            "message": {
                "accountKeys": [
                    { "pubkey": AUTHORITY, "signer": true, "writable": true },
                    { "pubkey": DEVICE, "signer": true, "writable": true }
                ],
                "instructions": [
                    {
                        "program": "system",
                        "programId": SYSTEM_PROGRAM,
                        "parsed": {
                            "type": "createAccount",
                            "info": {
                                "source": AUTHORITY,
                                "newAccount": DEVICE,
                                "lamports": 1_500_000,
                                "space": 80,
                                "owner": SYSTEM_PROGRAM
                            }
                        }
                    },
                    {
                        "program": "system",
                        "programId": SYSTEM_PROGRAM,
                        "parsed": {
                            "type": "initializeNonce",
                            "info": {
                                "nonceAccount": DEVICE,
                                "nonceAuthority": AUTHORITY,
                                "recentBlockhashesSysvar": RECENT_BLOCKHASHES_SYSVAR,
                                "rentSysvar": RENT_SYSVAR
                            }
                        }
                    }
                ]
            }
        }
    });
    vec![
        RpcStep {
            method: "getAccountInfo",
            result: account_info,
        },
        RpcStep {
            method: "getSignaturesForAddress",
            result: signatures,
        },
        RpcStep {
            method: "getTransaction",
            result: initialization,
        },
    ]
}

fn limits() -> PluginLimits {
    PluginLimits {
        call_fuel: 1_000_000_000,
        max_memory_bytes: 256 * 1024 * 1024,
        max_table_elements: 100_000,
        max_instances: 64,
    }
}

fn tool(
    env_key: &str,
    fallback_name: &str,
    permissions: Vec<PluginPermission>,
    config: HashMap<String, String>,
) -> Result<WasmTool> {
    let wasm_path = PathBuf::from(std::env::var(env_key).with_context(|| env_key.to_string())?);
    let bytes = std::fs::read(&wasm_path).with_context(|| wasm_path.display().to_string())?;
    let digest = zeroclaw_plugins::signature::sha256_hex(&bytes);
    WasmTool::from_wasm_with_digest(
        wasm_path,
        Some(&digest),
        permissions,
        fallback_name.to_string(),
        "ProofKiosk exact-host test".to_string(),
        config,
        limits(),
    )
}

#[tokio::test]
async fn proofkiosk_components_execute_through_exact_pinned_host() -> Result<()> {
    let merchant = std::env::var("PROOFKIOSK_MERCHANT").unwrap_or_else(|_| MERCHANT.to_string());
    let mint = std::env::var("PROOFKIOSK_MINT").unwrap_or_else(|_| MINT.to_string());
    let token_decimals =
        std::env::var("PROOFKIOSK_TOKEN_DECIMALS").unwrap_or_else(|_| "6".to_string());
    let price_list =
        std::env::var("PROOFKIOSK_PRICE_LIST").unwrap_or_else(|_| "cold_drink:1.5".to_string());
    let item_id = std::env::var("PROOFKIOSK_ITEM_ID").unwrap_or_else(|_| "cold_drink".to_string());
    let charge_config = HashMap::from([
        ("merchant_address".into(), merchant.clone()),
        ("usdc_mint".into(), mint.clone()),
        ("token_decimals".into(), token_decimals.clone()),
        ("price_list".into(), price_list.clone()),
        ("max_amount_usdc".into(), "10".into()),
    ]);
    let charge = tool(
        "PROOFKIOSK_CHARGE_WASM",
        "kiosk-charge",
        vec![PluginPermission::ConfigRead],
        charge_config,
    )?;
    ensure!(
        charge.name() == "kiosk_charge",
        "charge metadata export did not load"
    );
    ensure!(
        charge.parameters_schema()["additionalProperties"] == false,
        "charge schema boundary did not load"
    );
    let result = charge
        .execute(json!({
            "item_id": item_id,
            "__config": {
                "merchant_address": AUTHORITY,
                "usdc_mint": AUTHORITY,
                "token_decimals": "0",
                "price_list": "cold_drink:0"
            }
        }))
        .await?;
    ensure!(
        result.success,
        "charge execution failed: {:?}",
        result.error
    );
    if let Ok(output_path) = std::env::var("PROOFKIOSK_HOST_OUTPUT") {
        let wrapper = json!({
            "success": result.success,
            "output": &result.output,
            "error": &result.error,
        });
        let mut output_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .with_context(|| format!("refusing to overwrite host output {output_path}"))?;
        writeln!(output_file, "{}", serde_json::to_string(&wrapper)?)?;
        output_file.sync_all()?;
    }
    let output: Value = serde_json::from_str(&result.output)?;
    ensure!(output["v"] == 1 && output["status"] == "created");
    ensure!(
        output["recipient"] == merchant,
        "caller spoofed merchant config"
    );
    ensure!(output["mint"] == mint, "caller spoofed mint config");
    let reference = output["reference"]
        .as_str()
        .context("charge output has no reference")?
        .to_string();
    let charged_item = output["item_id"]
        .as_str()
        .context("charge output has no item_id")?
        .to_string();
    let charged_amount = output["amount"]
        .as_str()
        .context("charge output has no amount")?
        .to_string();
    ensure!(
        charged_item == item_id,
        "charge output changed the requested item"
    );

    let smuggled = charge
        .execute(json!({"item_id":"cold_drink", "merchant_address":AUTHORITY}))
        .await?;
    ensure!(
        !smuggled.success,
        "unknown model-facing field crossed the WIT boundary"
    );

    let decimals = token_decimals
        .parse::<u8>()
        .context("PROOFKIOSK_TOKEN_DECIMALS is not a u8")?;
    let watch_server = RpcMockServer::start(watch_steps(
        &merchant,
        &mint,
        decimals,
        &charged_amount,
        &reference,
        &charged_item,
    )?)
    .await?;
    let watch_config = HashMap::from([
        ("rpc_url".into(), watch_server.url.clone()),
        ("merchant_address".into(), merchant.clone()),
        ("usdc_mint".into(), mint.clone()),
        ("token_decimals".into(), token_decimals.clone()),
        ("price_list".into(), price_list.clone()),
        ("device_authority".into(), AUTHORITY.into()),
        ("device_address".into(), DEVICE.into()),
        ("device_id".into(), "kiosk-01".into()),
        ("finality".into(), "finalized".into()),
    ]);
    let watch = tool(
        "PROOFKIOSK_WATCH_WASM",
        "kiosk-watch",
        vec![PluginPermission::HttpClient, PluginPermission::ConfigRead],
        watch_config,
    )?;
    ensure!(
        watch.name() == "kiosk_watch",
        "watch metadata export did not load"
    );
    let watch_call = tokio::time::timeout(
        WASM_EXECUTION_TIMEOUT,
        watch.execute(json!({ "reference": reference, "item_id": charged_item })),
    )
    .await;
    let watch_server_result = watch_server.finish().await;
    let watch_result = watch_call.context("kiosk-watch Wasm execution timed out")??;
    let watch_requests = watch_server_result?;
    ensure!(
        watch_result.success,
        "valid payment was not accepted: {:?}",
        watch_result.error
    );
    if let Ok(output_path) = std::env::var("PROOFKIOSK_WATCH_OUTPUT") {
        let wrapper = json!({
            "success": watch_result.success,
            "output": &watch_result.output,
            "error": &watch_result.error,
        });
        let mut output_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .with_context(|| format!("refusing to overwrite watch output {output_path}"))?;
        writeln!(output_file, "{}", serde_json::to_string(&wrapper)?)?;
        output_file.sync_all()?;
    }
    let watch_output: Value = serde_json::from_str(&watch_result.output)?;
    ensure!(
        watch_output["v"] == 1
            && watch_output["success"] == true
            && watch_output["status"] == "paid"
            && watch_output["payment_verified"] == true,
        "watch did not emit the paid machine output: {}",
        watch_result.output
    );
    ensure!(watch_output["reference"] == reference);
    ensure!(watch_output["item_id"] == charged_item);
    ensure!(watch_output["signature"] == PAYMENT_SIGNATURE);
    ensure!(watch_output["payer"] == CUSTOMER);
    ensure!(watch_output["amount"] == charged_amount);
    ensure!(watch_output["recipient"] == merchant);
    ensure!(watch_output["mint"] == mint);
    ensure!(watch_output["token_decimals"] == decimals);
    ensure!(watch_output["payment_window_s"] == 900);
    ensure!(
        watch_output["payment_block_time_s"]
            .as_u64()
            .is_some_and(|value| value > 0),
        "watch did not preserve the verified payment block time"
    );
    ensure!(
        watch_requests.len() == 2,
        "watch made an unexpected RPC call count"
    );
    ensure!(watch_requests[0]["params"][0] == reference);
    ensure!(watch_requests[0]["params"][1]["commitment"] == "finalized");
    ensure!(watch_requests[1]["params"][0] == PAYMENT_SIGNATURE);
    ensure!(watch_requests[1]["params"][1]["commitment"] == "finalized");

    let invalid_watch = watch.execute(json!({})).await?;
    ensure!(
        !invalid_watch.success,
        "invalid watch request crossed the component boundary"
    );

    let attest_server = RpcMockServer::start(attest_steps()).await?;
    let attest_config = HashMap::from([
        ("rpc_url".into(), attest_server.url.clone()),
        ("device_id".into(), "kiosk-01".into()),
        ("nonce_account".into(), DEVICE.into()),
        ("nonce_authority".into(), AUTHORITY.into()),
        ("allowed_metrics".into(), "temp_c:-40:85".into()),
        ("custody_mode".into(), "t1".into()),
    ]);
    let attest = tool(
        "PROOFKIOSK_ATTEST_WASM",
        "kiosk-attest",
        vec![PluginPermission::HttpClient, PluginPermission::ConfigRead],
        attest_config,
    )?;
    ensure!(
        attest.name() == "kiosk_attest",
        "attest metadata export did not load"
    );
    let attest_call = tokio::time::timeout(
        WASM_EXECUTION_TIMEOUT,
        attest.execute(json!({ "kind": "reading", "metric": "temp_c", "value": 21.5 })),
    )
    .await;
    let attest_server_result = attest_server.finish().await;
    let attest_result = attest_call.context("kiosk-attest Wasm execution timed out")??;
    let attest_requests = attest_server_result?;
    ensure!(
        attest_result.success,
        "valid attestation was not built: {:?}",
        attest_result.error
    );
    let attest_output: Value = serde_json::from_str(&attest_result.output)?;
    ensure!(
        attest_output["v"] == 1
            && attest_output["success"] == true
            && attest_output["status"] == "signature_required"
            && attest_output["seq"] == 0,
        "attest did not emit its built-transaction machine output: {}",
        attest_result.output
    );
    ensure!(
        attest_output["unsigned_message_base64"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "attest returned no unsigned message"
    );
    ensure!(
        attest_requests.len() == 3,
        "attest made an unexpected RPC call count"
    );
    ensure!(attest_requests[0]["method"] == "getAccountInfo");
    ensure!(attest_requests[0]["params"][0] == DEVICE);
    ensure!(attest_requests[0]["params"][1]["commitment"] == "finalized");
    ensure!(attest_requests[0]["params"][1]["encoding"] == "base64");
    ensure!(attest_requests[1]["method"] == "getSignaturesForAddress");
    ensure!(attest_requests[1]["params"][0] == DEVICE);
    ensure!(
        attest_requests[1]["params"][1]["minContextSlot"] == NONCE_CONTEXT_SLOT,
        "attestation history request omitted the nonce snapshot minContextSlot"
    );
    ensure!(attest_requests[1]["params"][1]["commitment"] == "finalized");
    ensure!(attest_requests[2]["method"] == "getTransaction");
    ensure!(attest_requests[2]["params"][0] == NONCE_INIT_SIGNATURE);
    ensure!(attest_requests[2]["params"][1]["commitment"] == "finalized");

    let invalid_attest = attest.execute(json!({})).await?;
    ensure!(
        !invalid_attest.success,
        "invalid attest request crossed the component boundary"
    );

    println!(
        "exact pinned ZeroClaw executed charge, paid watch, and unsigned attestation paths; charge config spoof rejected"
    );
    Ok(())
}
