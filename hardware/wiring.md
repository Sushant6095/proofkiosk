# ProofKiosk — hardware wiring

Rung 3 of the [three-rung ladder](../README.md#the-three-rung-ladder). Rungs 1 and 2
need no hardware at all; this page is only for the physical kiosk.

**You do not need any of this to evaluate ProofKiosk.** The payment rail (rung 1) runs
on a laptop against localnet. Read this when you want a drink to actually drop.

## Bill of materials

| Part | Notes |
|---|---|
| Raspberry Pi 4 (2 GB+) | Any Pi with 40-pin header works; the Pi 4 is what this was built on. |
| 5 V relay module, opto-isolated, 1 channel | Opto-isolation is not optional — see the safety notes. |
| BME280 breakout (I²C) | Optional, rung 2 only. Temperature/humidity for the attestation loop. |
| 12 V solenoid / vend motor + its own 12 V supply | The load. Never powered from the Pi. |
| Flyback diode (1N4007) across the solenoid | Required for an inductive load. |
| Dupont jumpers, a common-ground wire | |

## Pin map

Physical pin numbers, BCM in parentheses. This is the mapping the SOPs and
`relay_pulse` examples assume; change it in one place if you wire differently.

| Signal | Pi pin | BCM | Goes to |
|---|---|---|---|
| Relay VCC | 2 | — | 5 V |
| Relay IN | 11 | GPIO17 | Relay control input |
| Relay GND | 6 | — | Ground |
| BME280 VIN | 1 | — | 3.3 V (**not** 5 V) |
| BME280 GND | 9 | — | Ground |
| BME280 SDA | 3 | GPIO2 | I²C data |
| BME280 SCL | 5 | GPIO3 | I²C clock |

```
                    Raspberry Pi 4
                   ┌───────────────┐
      5 V  pin 2 ──┤               ├── pin 1  3.3 V ──► BME280 VIN
                   │               │
   GPIO17 pin 11 ──┤               ├── pin 3  GPIO2 ──► BME280 SDA
                   │               │
      GND  pin 6 ──┤               ├── pin 5  GPIO3 ──► BME280 SCL
                   └───────────────┘
                          │
                          ▼
              ┌────────────────────────┐
              │  opto-isolated relay   │      12 V supply (separate!)
              │  VCC  IN  GND          │        │
              └───┬────┬────┬──────────┘        │
                  │    │    │                   ▼
                 5V  GPIO17 GND        ┌──────────────────┐
                                        │  solenoid / vend │
              relay COM ────────────────┤  motor           │
              relay NO  ──── 12 V ──────┤  (1N4007 flyback │
                                        │   across coil)   │
                                        └──────────────────┘
```

The relay's switched side (COM / NO / NC) is galvanically separate from the Pi side.
The 12 V loop closes through COM and NO only; 12 V must never touch a Pi pin.

## Safety notes — read these

1. **Use an opto-isolated relay board.** A bare relay coil driven from a GPIO pin will
   kick an inductive spike back into the Pi and eventually kill it. The opto stage is
   what keeps the two sides electrically apart.
2. **Give the load its own supply.** A solenoid inrush is amps; the Pi's 5 V rail is
   not. Share **ground** between the Pi and the 12 V supply, share nothing else.
3. **Flyback diode across every inductive load**, banded end to +12 V. Without it the
   relay contacts arc and weld shut — a welded contact is a dispenser stuck open.
4. **Active-low boards are common.** Many cheap relay modules energize on a LOW input,
   so they click on at boot while the GPIO is still floating. Test the polarity with
   the load disconnected before you wire anything that moves.
5. **Pulse, never hold.** `pin_ms = 400` in the payment-loop SOP is a pulse. A solenoid
   held energized will overheat. Match `pin_ms` to your mechanism's actual throw time.
6. **Fail-safe position.** Wire the dispenser so the de-energized state is *closed*.
   Power loss, a crashed agent, and a stuck process should all mean "nothing dispenses",
   never "everything dispenses".

## Calibration

The paper values above will not be right for your build; leave yourself the knob:

- **`pin_ms`** — start at 400 ms and adjust by observation. Too short and the mechanism
  half-throws; too long and the coil heats. This is per-mechanism, not per-design.
- **BME280 offset** — a breakout mounted near the Pi's SoC reads a few degrees high from
  board heat. Measure against a reference thermometer at steady state and record the
  offset. Attesting an uncorrected reading puts a known-wrong number on-chain
  permanently — the whole point of the attestation is that it is hard to walk back.
- **`allowed_metrics` bounds** in `config/example.toml` should bracket the *plausible*
  range of your enclosure, not the sensor's datasheet range. Bounds that are too wide
  stop catching a failing sensor.

## Building the host on the Pi

The stock ZeroClaw binary has no plugin host and no GPIO tools. Build from source with
both:

```bash
cargo build --release \
  --features plugins-wasm,plugins-wasm-cranelift,hardware,peripheral-rpi
```

- `plugins-wasm,plugins-wasm-cranelift` — the WIT plugin host that loads the three
  kiosk components. Needed on the laptop too.
- `hardware,peripheral-rpi` — the GPIO/peripheral tools (`relay_pulse`, sensor reads).
  Pi only.

Enable I²C for the BME280 (`raspi-config` → Interface Options → I²C), then confirm the
sensor is on the bus before wiring the relay:

```bash
i2cdetect -y 1        # expect 0x76 or 0x77
```

## Wiring it to the SOPs

`sops/payment-loop/` pulses the relay, and only on a verified payment. Before you
connect the load, read that SOP's **Known gap** section: the routing is verified but the
guard predicate is not yet wired end-to-end, so treat rung 3 as demo-wired rather than
production-wired, and keep a hand on the 12 V supply the first time you run it.
