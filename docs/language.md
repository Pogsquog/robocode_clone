# The robot language (robolang)

Robots are written in a small C-like language called **robolang** and saved
as `.bot` files. A `.bot` file is plain text: sharing a robot means sending
someone the file. Robot code runs inside an interpreter with **no access to
files, the network, the clock, or the operating system** — the only thing it
can do is call the battle functions listed below, within strict CPU and
memory budgets. Running a stranger's robot is completely safe.

## Quick start

```rust
// hello.bot: sweep the radar, aim, shoot.
func main() {
    set_radar_rate(45);              // sweep the radar continuously
    while (true) {
        var e = pop_event();         // check what happened
        if (e == "scanned") {
            // Someone is in the radar beam: swing the gun onto them and
            // fire as soon as it's there.
            fire_at(event_bearing(), 2);
        }
        await_tick();                // let one tick of battle happen
    }
}
```

Validate a robot without fighting: `tank check my.bot`

A robot may declare the language version it was written for on its first
non-blank line; engines reject robots written for a newer version:

```
// robolang 1
```

## The language

### Program shape

A program is a set of functions. Execution starts at `main`, which normally
never returns: `main` runs *forever*, pausing at blocking operations while
the battle happens around it.

```
func name(param1, param2) { ... return value; }
```

- `main` must exist and takes no parameters.
- Recursion is allowed (call depth is capped).
- Variables declared with `var` **anywhere in `main` are global** — they
  persist across ticks and are visible inside every function. This is where
  your robot's memory lives. Declaring the same name again in `main` (say,
  two `for (var i = ...)` loops) reuses the same global.
- `var` inside other functions (and function parameters) is local.
- `var x;` with no initializer sets `x` to `null` every time it runs.

### Types and expressions

- **Numbers** (`42`, `3.5`), **booleans** (`true`, `false`), **strings**
  (`"hit!"`). Strings exist for `log()` and `event_name()`.
- Arithmetic: `+ - * / %`. Division or modulo by zero **forfeits your robot**.
- `+` also concatenates: `"energy: " + energy()`.
- Comparisons: `== != < <= > >=`, logic: `&& || !` (short-circuit).
- Conditions are permissive: `0`, `""`, `false`, `null` are falsy.
- Statements: `var x = 1;`, `x += 5;` (also `-=`, `*=`, `/=`),
  `if / else`, `while`, `for (var i = 0; i < 10; i += 1) { ... }`,
  `break;`, `continue;`, `return;` / `return expr;`.
- Comments: `// line` and `/* block */`.

### Arrays

Arrays hold a robot's longer-term memory: a history of enemy positions, a
histogram of where its shots should have gone, a grid of the arena.

```
func main() {
    var hist[32];                  // 32 numbers, all starting at 0
    var head = 0;
    ...
    hist[head] = event_x();        // store
    head = (head + 1) % len(hist); // len() is the declared size
    avg_x = (hist[0] + hist[1]) / 2;
}
```

Arrays are deliberately simple:

- **Declared at the top level of `main`** with a literal size:
  `var name[N];`. Like other variables in `main` they are global: every
  function can use them by name. They are created once, when the robot
  starts, with every element `0`; the declaration line itself does
  nothing when it runs.
- **Numbers only.** Storing a string, boolean or null forfeits the robot.
- **Indexed from 0** with whole numbers: `a[i]`, `a[i] = v`, and
  `a[i] += v` (also `-=`, `*=`, `/=`). An index that is out of range or
  not a whole number forfeits the robot.
- **Not values.** An array can't be copied, compared, logged, passed to a
  function or returned; only its elements can be used. For a grid, use
  one array with `grid[y * width + x]`.

See `examples/learner.bot` for a gun that learns an enemy's movement with
two arrays: a ring buffer of "virtual waves" and a histogram.

### The sandbox

| Limit | Value |
|---|---|
| VM instructions per tick | 5,000 |
| Budget overruns allowed | 30 (then forfeit) |
| Call depth | 96 |
| Value stack | 4,096 |
| Globals / locals / functions | 256 / 256 / 64 |
| Array elements (all arrays together) | 16,384 |
| Nesting depth (blocks, parentheses, operator chains) | 100 |
| String length | 256 bytes |
| Source size | 256 KB |

An infinite loop that never calls a blocking function burns the instruction
budget every tick; after 30 strikes the robot forfeits. The budget covers
the whole tick: a blocking call that finishes instantly (`ahead(0)`,
`turn_gun(0)`) resumes your code in the same tick and keeps spending it. An infinite loop
that *does* block (`while (true) { ... await_tick(); }`) is the normal way
to write a robot.

A runtime fault (bad types, division by zero, calling `ahead("fast")`,
passing NaN or infinity to a battle function, e.g. `fire(sqrt(-1))`)
forfeits the robot immediately. Comparisons involving NaN are simply
`false`. Compile errors are reported by `tank check`
before any battle starts.

## Battle model

- Fixed timestep. The arena is a rectangle (default 1000 x 700).
- Angles are **degrees**, 0 = north (up), increasing **clockwise**.
- Your tank has three rotating parts, mounted in a chain:
  **body -> gun -> radar**. Turning the body carries the gun and radar;
  turning the gun carries the radar.
- Energy starts at 100. Bullets, wall hits and collisions cost energy.
  Last robot standing wins. At the tick limit, highest energy wins.

### Movement and turning

Two styles, freely mixable:

**Blocking style** — suspends your code until the motion finishes:

| Function | Effect |
|---|---|
| `ahead(dist)` | drive forward `dist` units |
| `back(dist)` | drive backward |
| `turn_body(deg)` | rotate the body |
| `turn_gun(deg)` | rotate the gun (relative to body) |
| `turn_radar(deg)` | rotate the radar (relative to gun) |
| `await_tick()` | suspend for exactly one tick |

**Advanced style** — set rates/velocity that persist every tick:

| Function | Effect |
|---|---|
| `set_velocity(v)` | target speed, -8..8 units/tick |
| `set_body_rate(deg)` | body deg/tick, max 10 (less at speed) |
| `set_gun_rate(deg)` | gun deg/tick, max 20 |
| `set_radar_rate(deg)` | radar deg/tick, max 45 |

Acceleration is 2 units/tick, braking is 4. Blocking moves brake in time
to stop on the requested distance, even when called at speed or while
moving the other way; the one exception is a distance shorter than the
tank's minimum braking distance. While a blocking motion is in
progress, your code cannot run — use the advanced style if you need to react
every tick (that is what good combat bots do).

### Radar

The radar is a narrow (18°) beam with unlimited range. Each tick the engine
checks every enemy against the **entire arc your beam swept that tick**, so
a fast sweep cannot jump over a target. If an enemy is in the swept arc you
get a `scanned` event for the **nearest** one — an active radar lock.
Sweeping strategy matters: a stationary beam sees nothing, a blind orbiting
sweep only spots enemies once per revolution, and *locking* the radar on a
found target (see `tracker.bot`) rescans it every tick.

### Guns and bullets

There are two ways to shoot:

- **`fire_at(heading, power)`**: the easy, accurate way. At the end of
  this tick the engine turns the gun onto the absolute `heading`, as far as
  the gun can turn in one tick (20°), and fires along it if the gun got
  there, is cool, and you can afford the shot. It fires exactly on the
  heading, so there's no aiming error to allow for. If the heading is out
  of reach or the gun is still hot, the gun just turns toward it. The
  request lasts one tick, so call it every tick you want the gun on target
  (it also keeps the gun aimed while it cools). It overrides
  `set_gun_rate` for that tick; it has no effect while a `turn_gun()` is in
  progress.
- **`fire(power)`**: fires immediately along wherever the gun points now, if
  the gun is cool and you can afford it; returns `true`/`false`. Use it
  with your own gun steering (`set_gun_rate`, `turn_gun`).

| Power | 0.1 .. 3.0 (values are clamped) |
|---|---|
| Energy cost | = power |
| Damage | 4 x power |
| Bullet speed | 20 - 4 x power (slow, heavy shots hit hard) |
| Cooldown | 5 + 5 x power ticks |
| Reward | a hit refunds 3 x power energy to the shooter |

Bullets are dodgeable: at 400 units, a power-2 bullet flies for 25 ticks.

### Events

Events queue up; `pop_event()` returns the next one as a string, or `false`
if the queue is empty. After popping, read the event's details with the
`event_*` accessors (they describe the **last popped** event).

| Kind | When | Useful fields |
|---|---|---|
| `"scanned"` | radar swept over an enemy | all fields |
| `"bullet_hit"` | your bullet hit someone | `event_x/y`, `event_power` |
| `"hit_by_bullet"` | you were hit | `event_bearing`, `event_power` |
| `"bullet_missed"` | your bullet left the arena | |
| `"wall"` | you hit a wall (your move stops) | `event_bearing` |
| `"robot_collision"` | you bumped a robot | `event_bearing`, `event_name` |
| `"robot_death"` | an enemy died | `event_name` |

Events become visible the tick after they happen.

### State getters

`x()`, `y()`, `velocity()`, `energy()`, `body_heading()`, `gun_heading()`
(absolute), `radar_heading()` (absolute), `gun_heat()` (ticks until cool),
`time()` (tick counter), `arena_w()`, `arena_h()`.

### Math helpers

`sin(deg)`, `cos(deg)`, `abs(n)`, `min(a,b)`, `max(a,b)`, `sqrt(n)`,
`norm_deg(deg)` (normalize to 0..360), `bearing_to(x, y)` (absolute bearing
from you to a point — the key to aiming), `atan2(y, x)`, `log(anything)`
(debug output, shown by `--verbose` and in the replay viewer; up to 500
lines per robot).

`atan2(y, x)` is ordinary maths in degrees (-180..180), the inverse of
`sin` and `cos`: `atan2(sin(a), cos(a))` gives back `a`. Two common uses:

```
// Heading (0 = north, clockwise) of a movement vector (vx, vy):
var h = norm_deg(atan2(vx, 0 - vy));
// Angular half-width of a tank (radius 18) seen from distance d:
var half = atan2(18, d);
```

### Aiming recipe

Point at where the target is, and let `fire_at` do the rest:

```
fire_at(bearing_to(event_x(), event_y()), 2);
```

For moving targets, predict where they will be when the bullet arrives and
aim there instead (see `examples/sniper.bot`), or learn where they tend to
go (see `examples/learner.bot`).

## Example robots

| File | Strategy |
|---|---|
| `examples/sweeper.bot` | orbiting radar, fire on sight, drive in arcs |
| `examples/corner.bot` | blocking navigation to a corner, then radar lock |
| `examples/tracker.bot` | radar + gun lock, continuous fire, dodge wiggle |
| `examples/sniper.bot` | stationary lead-targeting sharpshooter |
| `examples/learner.bot` | statistical ("guess factor") gun that learns movement patterns, using arrays |
| `examples/rammer.bot` | charges on a curving path and fires heavy shots point-blank |
