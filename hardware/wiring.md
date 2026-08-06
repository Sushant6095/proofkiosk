# ProofKiosk — hardware wiring

Rung 3 of the [three-rung ladder](../README.md#the-three-rung-ladder). Rungs 1 and 2
need no hardware at all; this page is only for the physical kiosk.

**You do not need any of this to evaluate the payment plugins.** The payment rail runs on
a laptop/localnet. The repository now includes `scripts/actuator.mjs`: a fixed-BCM17,
400 ms, cooldown-enforcing host driver behind raw-result validation and an exclusive
claim. Its automated test uses a fake GPIO executable. This page is still a reference
design—not evidence of a particular relay/load build or sensor-backed delivery.

## Bill of materials

| Part | Notes |
|---|---|
| Raspberry Pi 4 (2 GB+) | Reference target only; the checked-in CI does not run on a Pi. |
| 3.3 V-compatible isolated relay/driver, 1 channel | Select from its datasheet for the exact load; “opto-isolated” on a listing is not proof of end-to-end isolation. |
| BME280 breakout (I²C) | Optional, rung 2 only. Temperature/humidity for the attestation loop. |
| 12 V solenoid / vend motor + its own 12 V supply | The load. Never powered from the Pi. |
| Flyback diode (1N4007) across the solenoid | Required for an inductive load. |
| Fused terminals, enclosure, emergency disconnect | Do not prototype an energized inductive load on loose Dupont wiring. |

## Pin map

Physical pin numbers, BCM in parentheses. `scripts/actuator.mjs` binds the control output
to BCM17; relay polarity must be verified with the load disconnected because the current
driver assumes active-high operation.

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

The diagram is conceptual and does not specify a relay module's coil/JD-VCC topology.
The dry-contact side (COM / NO / NC) must remain separate from Pi logic, and the complete
12 V loop—including its return—is confined to that side. Verify the selected module's
datasheet; 12 V must never touch a Pi pin.

## Safety notes — read these

1. **Use a reviewed 3.3 V-compatible driver.** Never drive a relay coil or motor from a
   GPIO. An optocoupler label alone is insufficient: many boards bridge grounds or coil
   supply unless their isolation jumper/topology is configured correctly.
2. **Give the load its own supply.** A solenoid inrush is amps; the Pi's 5 V rail is
   not. Do **not** blindly share grounds: a genuinely isolated interface keeps logic and
   load grounds separate, while a non-isolated MOSFET design requires a common reference.
   Follow the exact reviewed driver schematic rather than mixing the two topologies.
3. **Flyback diode across every inductive load**, banded end to +12 V. Without it the
   relay contacts arc and weld shut — a welded contact is a dispenser stuck open.
4. **Active-low boards are common.** Many cheap relay modules energize on a LOW input,
   so they click on at boot while the GPIO is still floating. Test the polarity with
   the load disconnected before you wire anything that moves.
5. **Pulse in a non-LLM safety adapter, never with two unconstrained model-callable GPIO
   calls.** `scripts/actuator.mjs` fixes BCM17 and 400 ms, caps the GPIO command timeout,
   preflights LOW, uses a host-wide lock/cooldown, and forces LOW in `finally`. A hardware
   monostable/watchdog is still required before a production inductive load because a
   killed or frozen OS process cannot itself guarantee de-energization.
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

The stock ZeroClaw binary has no plugin host. Build the exact compatible source pin with
the plugin runtime plus Pi features:

```bash
ZEROCLAW_FEATURES=plugins-wasm-cranelift,hardware,peripheral-rpi \
  ./scripts/install-pinned-zeroclaw.sh
```

- `plugins-wasm-cranelift` — the WIT plugin host that loads the three
  kiosk components. Needed on the laptop too.
- `hardware,peripheral-rpi` — ZeroClaw's lower-level GPIO/peripheral support. ProofKiosk's
  trusted actuator remains a separate, non-model-callable host process using `pinctrl`
  (Pi OS Bookworm) or `raspi-gpio`; no `relay_pulse` tool is exposed to the agent.

Enable I²C for the BME280 (`raspi-config` → Interface Options → I²C), then confirm the
sensor is on the bus before wiring the relay:

```bash
i2cdetect -y 1        # expect 0x76 or 0x77
```

## Wiring it to the SOPs

`sops/payment-loop/` specifies the desired guard and pulse contract; validation alone does
not execute the relay. The external path is raw host-direct watch result → immutable order
comparison → host lock/cooldown → exclusive claim → `actuating` → fixed GPIO pulse →
`pulse_completed`. It is invoked outside the model with `scripts/actuator.mjs`; the
pinned headless SOP does not automatically dispatch it.

`pulse_completed` is deliberately not `delivered`. A crash after the claim, a mechanical
jam, or an item failing to fall requires operator recovery. Before a production load, add
a hardware one-shot/watchdog and a sensor-backed `delivered`/`jammed` transition. Until
then, rehearse with an LED/current-limited load and keep a reachable disconnect.
