# tank

A robocode-style programming game. You write the AI for a tank in a small
sandboxed language, pit robots against each other, and watch the replay in
a browser.

Written in Rust: the engine and robot language compile to a fast native
binary, and the viewer is a static HTML/JS page.

## Requirements

- [Rust](https://rustup.rs) (stable)
- Any modern browser to watch replays

## Quick start

```
cargo build --release
./target/release/tank run examples/sweeper.bot examples/tracker.bot --seed 42 --out battle.json
# then open viewer/index.html in a browser and load battle.json
```

## What's in the box

- **Tanks** with independently steered body, gun, and radar.
- **Narrow active radar** (18° beam): a sweeping beam discovers opponents;
  it locks the nearest enemy in the swept arc and reports full contact data.
- **Finite-speed bullets**: weak shots fly fast, heavy shots hit hard and
  travel slowly — dodging and lead-aiming both work.
- **Energy model**: firing costs energy, hits refund some; last tank
  standing wins.
- **Sandboxed robot code**: robots are written in `robolang`, a purpose-built
  interpreted language with **no I/O of any kind** and hard CPU/memory
  budgets. A `.bot` file is plain text and is always safe to run — the
  interpreter physically cannot express file, network, or system access.
- **Fully deterministic battles**: same robots + same seed = byte-identical
  replay. Every battle outcome can be reproduced and verified.
- **Headless engine + static viewer**: the CLI simulates and writes a JSON
  replay; `viewer/index.html` (plain HTML/JS, no server, no build) plays it
  back with play/pause, scrubbing, speed control, and an event log.

## Running robots yourself

The `examples/` directory has robots of varied skill, and the safe language
means community bots can be shared without risk. See
[docs/language.md](docs/language.md) to write your own.

## Layout

```
crates/robolang/   the robot language: lexer, parser, bytecode, sandboxed VM
crates/engine/     deterministic battle simulation (physics, radar, bullets)
crates/tank-cli/   `tank` command: run battles, validate robots, write replays
viewer/            static replay viewer (open index.html, drop in a replay)
examples/          example robots of varied skill
docs/language.md   the robot language reference
```

## CLI

```
tank run <a.bot> <b.bot> [more...] [options]   fight robots, write a replay
tank check <a.bot> [more...]                   validate robot source

options: --seed N   --max-ticks N   --out FILE   --verbose   --quiet
```

Battles support more than two robots: `tank run a.bot b.bot c.bot`.

## Safety model

Robot code is compiled to bytecode and run by a stack VM whose only
interface to the outside world is a fixed table of ~40 battle functions
(`fire`, `radar_heading`, ...). There is no escape hatch: no imports, no
syscalls, no reflection. Resource abuse is bounded by the instruction
budget (5,000 ops/tick), call-depth and stack caps, event-queue and log
caps. A robot that misbehaves is simply disabled; it can never harm the
host, stall the engine, or read anything.

## Determinism

The simulation uses fixed update order (robots always processed by id), a
seeded splitmix64 RNG, and f64 arithmetic throughout, so a given engine
build reproduces any battle bit-for-bit from its seed. CI includes a test
that runs the same battle twice (including as two separate OS processes)
and asserts the replay files are byte-identical. Cross-*build* bit-exact
replay (different compilers/platforms) is not promised; if ever needed, the
path is fixed-point math, not floats.

## Tuning

All balance constants (speeds, turn rates, damage formulas, cooldowns,
budgets) live in one place: `crates/engine/src/config.rs`.
