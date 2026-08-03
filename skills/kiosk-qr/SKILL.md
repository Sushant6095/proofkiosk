# kiosk-qr — render a Solana Pay charge as a scannable QR (host-side)

A tiny **host-side** helper for ProofKiosk. It is deliberately **not** part of any
wasm plugin: `kiosk-charge` stays a small, zero-network component. The helper accepts
the **raw host-direct ToolResult**, validates its recipient, mint, decimals, catalog
price, reference and PKPAY1 memo against operator config, persists the order, and only
then turns the validated URL into a QR image.

## When to use it

After a trusted host runner captures `kiosk_charge`'s raw ToolResult, use this to give
the customer something to scan or tap. Do not feed it `zc agent` prose: an LLM can
fabricate JSON, so result provenance must remain outside the model boundary.

- **QR image** — for a customer standing at the kiosk looking at a *separate* screen
  (the Pi's display, or a photo sent into the chat). A QR only makes sense across two
  screens; a customer on the same phone should use the tap-link instead.
- **Tap-link fallback** — the `solana:` URI itself is tappable: mobile wallets
  (Phantom, Solflare, …) register the `solana:` scheme and open the payment pre-filled
  with one tap, for same-device chat flows. No wrapper service needed.

## Usage

```bash
./render-qr.sh /trusted-trace/kiosk-charge-result.json /etc/zeroclaw/config.toml out.png
# -> rejects any config/output/URL mismatch
# -> writes a mode-0600 order under .proofkiosk/orders/
# -> writes out.png and prints the exact validated tap-link
```

The first file must be either the exact machine-output JSON or the exact WIT ToolResult
wrapper (`success`, `output`, `error`) captured directly by the host. Never paste a URL
or copy a model response into it. `render-qr.sh` uses `qrencode` if present; validation
and order persistence use the repository's Node.js helper and need no network.

## Delivery

- Channels that support images (Telegram `sendPhoto`, Discord, WhatsApp, Matrix) send
  `out.png` as a photo with the amount in the caption.
- Text-only channels (IRC, email) send the tap-link and the raw `solana:` URL.

The image and link are presentation only—the customer's wallet still signs the
transfer. The handoff holds no key and cannot change the configured amount. After
`kiosk-watch` verifies payment, a trusted driver must run `trusted-order-claim.mjs`
against a host-direct Watch ToolResult before any actuator pulse. That claim is
durable and once-only on one host; it is not an actuator or a complete crash-recovery
state machine.
