//! The battle: fixed-order deterministic tick loop.
//!
//! Tick phases (always in this order, robots always iterated by id):
//! 1. VM phase: resume robots whose blocking op completed, run their code
//!    within the instruction budget, collect intents (bullets may spawn).
//! 2. Movement phase: cool the gun, apply pending/intent turn rates and
//!    velocity, integrate positions, resolve wall and robot collisions.
//! 3. Bullet phase: advance bullets, resolve hits.
//! 4. Death/end phase: apply deaths, decide the winner.
//! 5. Radar phase: sweep beams, queue Scanned events (readable next tick).
//! 6. Snapshot phase: record replay data.

use crate::config::Config;
use crate::event::{Event, EventKind};
use crate::host_impl::RobotHost;
use crate::replay::Snapshot;
use crate::rng::Rng;
use crate::robot::{ang_diff, bearing_deg, dist, norm_deg, point_segment_dist, Pending, Robot};
use robolang::{self, BlockRequest, RunOutcome, Value, Vm};
use std::collections::VecDeque;

pub struct Bullet {
    pub owner: usize,
    pub x: f64,
    pub y: f64,
    /// Absolute heading in degrees.
    pub heading: f64,
    pub speed: f64,
    pub power: f64,
    pub spawn_tick: u32,
    pub alive: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EndReason {
    LastStanding,
    TickLimit,
}

#[derive(Clone, Debug)]
pub struct BattleResult {
    /// Index of the winning robot, or None for a draw.
    pub winner: Option<usize>,
    pub reason: EndReason,
    pub tick: u32,
}

/// A robot spec: name + source code.
pub struct RobotSpec {
    pub name: String,
    pub source: String,
}

pub struct Battle {
    pub cfg: Config,
    pub robots: Vec<Robot>,
    pub bullets: Vec<Bullet>,
    pub tick: u32,
    pub max_ticks: u32,
    pub rng: Rng,
    pub seed: u64,
    pub result: Option<BattleResult>,
    /// Per-tick human-readable events for the replay (hits, deaths, logs...).
    pub tick_log: Vec<String>,
    pub snapshots: Vec<Snapshot>,
    /// Robot log() output: (tick, robot id, message).
    pub logs: Vec<(u32, usize, String)>,
}

impl Battle {
    /// Create a battle from robot sources. Compilation failures are returned
    /// as errors naming the offending robot.
    pub fn new(
        cfg: Config,
        specs: &[RobotSpec],
        seed: u64,
        max_ticks: u32,
    ) -> Result<Battle, String> {
        if specs.len() < 2 {
            return Err("a battle needs at least two robots".into());
        }
        let mut robots = Vec::with_capacity(specs.len());
        let mut rng = Rng::new(seed);
        for (i, spec) in specs.iter().enumerate() {
            let prog = robolang::compile(&spec.source)
                .map_err(|e| format!("robot '{}' failed to compile: {} (line {})", spec.name, e.msg, e.line))?;
            robots.push(Robot {
                id: i,
                name: spec.name.clone(),
                body_heading: 0.0,
                gun_rel: 0.0,
                radar_rel: 0.0,
                prev_radar_heading: 0.0,
                x: 0.0,
                y: 0.0,
                velocity: 0.0,
                energy: cfg.start_energy,
                gun_heat: 0.0,
                alive: true,
                fault: None,
                intent_velocity: 0.0,
                intent_body_rate: 0.0,
                intent_gun_rate: 0.0,
                intent_radar_rate: 0.0,
                pending: None,
                vm: Some(Vm::new(prog)),
                events: VecDeque::new(),
                current_event: None,
                stats: Default::default(),
            });
        }
        place_robots(&cfg, &mut robots, &mut rng);
        Ok(Battle {
            cfg,
            robots,
            bullets: Vec::new(),
            tick: 0,
            max_ticks,
            rng,
            seed,
            result: None,
            tick_log: Vec::new(),
            snapshots: Vec::new(),
            logs: Vec::new(),
        })
    }

    /// Run to completion (or the tick limit).
    pub fn run(&mut self) {
        while self.result.is_none() && self.tick < self.max_ticks {
            self.step();
        }
        if self.result.is_none() {
            self.finish(EndReason::TickLimit);
        }
    }

    /// Run a single tick.
    pub fn step(&mut self) {
        self.tick_log.clear();
        self.vm_phase();
        self.movement_phase();
        self.bullet_phase();
        self.death_phase();
        self.radar_phase();
        self.snapshot_phase();
        self.tick += 1;
    }

    // ----- Phase 1: robot code ------------------------------------------------

    fn vm_phase(&mut self) {
        let budget = self.cfg.instruction_budget;
        for i in 0..self.robots.len() {
            if !self.robots[i].alive {
                continue;
            }
            // If a blocking op just completed, resume the VM with its result.
            if let Some(p) = self.robots[i].pending {
                if pending_complete(&p, &self.cfg) {
                    let r = self.robots[i].vm.take();
                    self.robots[i].pending = None;
                    if let Some(mut vm) = r {
                        vm.resume(Value::Null);
                        self.robots[i].vm = Some(vm);
                    }
                } else {
                    continue; // still blocked; VM idles this tick
                }
            }
            self.run_vm(i, budget);
        }
    }

    fn run_vm(&mut self, i: usize, budget: u32) {
        let start = match &self.robots[i].vm {
            Some(vm) => vm.ops_executed(),
            None => return, // halted earlier
        };
        loop {
            let Some(mut vm) = self.robots[i].vm.take() else {
                return;
            };
            // One budget per tick, shared across trivial blocking calls that
            // complete instantly (e.g. `ahead(0)`) and resume the VM below.
            let used = vm.ops_executed() - start;
            let remaining = u64::from(budget).saturating_sub(used) as u32;
            let outcome = {
                let mut host = RobotHost::new(self, i);
                vm.run(&mut host, remaining)
            };
            self.robots[i].vm = Some(vm);
            match outcome {
                RunOutcome::Halted => {
                    self.robots[i].vm = None;
                    self.tick_log
                        .push(format!("{}'s program ended; it idles", self.robots[i].name));
                }
                RunOutcome::Fault(msg) => {
                    self.forfeit(i, format!("runtime fault: {}", msg));
                    return;
                }
                RunOutcome::BudgetExceeded => {
                    self.robots[i].stats.budget_strikes += 1;
                    if self.robots[i].stats.budget_strikes >= self.cfg.max_budget_strikes {
                        self.forfeit(
                            i,
                            format!(
                                "exceeded the instruction budget {} times",
                                self.cfg.max_budget_strikes
                            ),
                        );
                        return;
                    }
                    return; // try again next tick
                }
                RunOutcome::Blocked(req) => {
                    match self.make_pending(req) {
                        Some(p) => {
                            self.robots[i].pending = Some(p);
                        }
                        None => {
                            // Trivial request (e.g. ahead(0)): complete now.
                            let mut vm = self.robots[i].vm.take().unwrap();
                            vm.resume(Value::Null);
                            self.robots[i].vm = Some(vm);
                            continue;
                        }
                    }
                    return;
                }
            }
        }
    }

    fn make_pending(&self, req: BlockRequest) -> Option<Pending> {
        let eps = self.cfg.epsilon;
        match req {
            BlockRequest::Ahead(d) => {
                if d.abs() < eps {
                    None
                } else {
                    Some(Pending::Move {
                        remaining: d.abs(),
                        dir: d.signum(),
                    })
                }
            }
            BlockRequest::Back(d) => {
                if d.abs() < eps {
                    None
                } else {
                    Some(Pending::Move {
                        remaining: d.abs(),
                        dir: -d.signum(),
                    })
                }
            }
            BlockRequest::TurnBody(d) => {
                if d.abs() < eps {
                    None
                } else {
                    Some(Pending::TurnBody { remaining: d })
                }
            }
            BlockRequest::TurnGun(d) => {
                if d.abs() < eps {
                    None
                } else {
                    Some(Pending::TurnGun { remaining: d })
                }
            }
            BlockRequest::TurnRadar(d) => {
                if d.abs() < eps {
                    None
                } else {
                    Some(Pending::TurnRadar { remaining: d })
                }
            }
            BlockRequest::AwaitTick => Some(Pending::AwaitTick { n: 1 }),
        }
    }

    fn forfeit(&mut self, i: usize, reason: String) {
        self.robots[i].alive = false;
        self.robots[i].fault = Some(reason.clone());
        self.robots[i].pending = None;
        self.robots[i].vm = None;
        self.tick_log
            .push(format!("{} forfeits: {}", self.robots[i].name, reason));
    }

    // ----- Phase 2: movement --------------------------------------------------

    fn movement_phase(&mut self) {
        let cfg = self.cfg.clone();
        for i in 0..self.robots.len() {
            if !self.robots[i].alive {
                continue;
            }
            let r = &mut self.robots[i];

            // Remember where the radar beam started this tick (it may have
            // been carried around by body/gun rotation as well as its own).
            r.prev_radar_heading = r.radar_heading();

            // Gun cooling.
            r.gun_heat = (r.gun_heat - 1.0).max(0.0);

            // AwaitTick pending counts down here so it always spans exactly
            // one full simulation tick.
            if let Some(Pending::AwaitTick { n }) = &mut r.pending {
                *n -= 1;
            }

            // Body rotation.
            let body_limit = r.body_rate_limit(&cfg);
            let body_rate = match &mut r.pending {
                Some(Pending::TurnBody { remaining }) => {
                    let d = remaining.clamp(-body_limit, body_limit);
                    *remaining -= d;
                    d
                }
                _ => r.intent_body_rate.clamp(-body_limit, body_limit),
            };
            r.body_heading = norm_deg(r.body_heading + body_rate);

            // Gun rotation (relative to body).
            let gun_rate = match &mut r.pending {
                Some(Pending::TurnGun { remaining }) => {
                    let d = remaining.clamp(-cfg.max_gun_rate, cfg.max_gun_rate);
                    *remaining -= d;
                    d
                }
                _ => r.intent_gun_rate.clamp(-cfg.max_gun_rate, cfg.max_gun_rate),
            };
            r.gun_rel += gun_rate;

            // Radar rotation (relative to gun).
            let radar_rate = match &mut r.pending {
                Some(Pending::TurnRadar { remaining }) => {
                    let d = remaining.clamp(-cfg.max_radar_rate, cfg.max_radar_rate);
                    *remaining -= d;
                    d
                }
                _ => r.intent_radar_rate.clamp(-cfg.max_radar_rate, cfg.max_radar_rate),
            };
            r.radar_rel += radar_rate;

            // Velocity: pending movement drives the target speed, slowing
            // in time to stop at the requested distance.
            let target = match r.pending {
                Some(Pending::Move { remaining, dir }) => dir * move_speed(remaining, &cfg),
                _ => r.intent_velocity.clamp(-cfg.max_speed, cfg.max_speed),
            };
            r.velocity = next_velocity(r.velocity, target, &cfg);
            // Integrate position along the body heading.
            let h = r.body_heading.to_radians();
            r.x += r.velocity * h.sin();
            r.y -= r.velocity * h.cos();

            // Consume pending movement. Travel in the wrong direction (still
            // braking from earlier motion) adds to the distance left.
            if let Some(Pending::Move { remaining, dir }) = &mut r.pending {
                *remaining -= r.velocity * *dir;
            }

            // Wall collision.
            let margin = cfg.tank_radius;
            let (clamped_x, clamped_y) = (
                r.x.clamp(margin, cfg.arena_w - margin),
                r.y.clamp(margin, cfg.arena_h - margin),
            );
            if clamped_x != r.x || clamped_y != r.y {
                r.x = clamped_x;
                r.y = clamped_y;
                r.velocity = 0.0;
                let bearing = r.body_heading;
                if matches!(r.pending, Some(Pending::Move { .. })) {
                    r.pending = None; // wall stops the move
                }
                self.take_damage(i, cfg.wall_damage);
                let mut e = Event::empty(EventKind::Wall);
                e.bearing = bearing;
                self.robots[i].queue_event(&cfg, e);
                self.tick_log
                    .push(format!("{} hit a wall", self.robots[i].name));
            }
        }

        // Robot-robot collisions (pairwise, deterministic order).
        let n = self.robots.len();
        for a in 0..n {
            for b in (a + 1)..n {
                if !self.robots[a].alive || !self.robots[b].alive {
                    continue;
                }
                let min_d = self.cfg.tank_radius * 2.0;
                let (ax, ay) = (self.robots[a].x, self.robots[a].y);
                let (bx, by) = (self.robots[b].x, self.robots[b].y);
                let d = dist(ax, ay, bx, by);
                if d < min_d {
                    // Push apart along the separation axis.
                    let (dx, dy) = if d > self.cfg.epsilon {
                        ((bx - ax) / d, (by - ay) / d)
                    } else {
                        (1.0, 0.0)
                    };
                    let overlap = (min_d - d) / 2.0 + self.cfg.epsilon;
                    self.robots[a].x -= dx * overlap;
                    self.robots[a].y -= dy * overlap;
                    self.robots[b].x += dx * overlap;
                    self.robots[b].y += dy * overlap;
                    // Clamp both back inside the arena.
                    let m = self.cfg.tank_radius;
                    for idx in [a, b] {
                        self.robots[idx].x =
                            self.robots[idx].x.clamp(m, self.cfg.arena_w - m);
                        self.robots[idx].y =
                            self.robots[idx].y.clamp(m, self.cfg.arena_h - m);
                        self.robots[idx].velocity = 0.0;
                        if matches!(self.robots[idx].pending, Some(Pending::Move { .. })) {
                            self.robots[idx].pending = None;
                        }
                    }
                    for (me, other) in [(a, b), (b, a)] {
                        let mut e = Event::empty(EventKind::RobotCollision);
                        e.x = self.robots[other].x;
                        e.y = self.robots[other].y;
                        e.bearing = bearing_deg(
                            self.robots[me].x,
                            self.robots[me].y,
                            self.robots[other].x,
                            self.robots[other].y,
                        );
                        e.dist = dist(
                            self.robots[me].x,
                            self.robots[me].y,
                            self.robots[other].x,
                            self.robots[other].y,
                        );
                        e.name = self.robots[other].name.clone();
                        self.robots[me].queue_event(&self.cfg, e);
                    }
                    self.take_damage(a, self.cfg.collision_damage);
                    self.take_damage(b, self.cfg.collision_damage);
                    self.tick_log.push(format!(
                        "{} and {} collided",
                        self.robots[a].name, self.robots[b].name
                    ));
                }
            }
        }
    }

    // ----- Phase 3: bullets ---------------------------------------------------

    fn bullet_phase(&mut self) {
        let cfg = self.cfg.clone();
        let tick = self.tick;
        for bi in 0..self.bullets.len() {
            // Copy bullet state out to avoid borrows across mutations.
            let (b_alive, b_spawn, b_owner, b_power, b_x, b_y, b_heading, b_speed) = {
                let b = &self.bullets[bi];
                (
                    b.alive,
                    b.spawn_tick,
                    b.owner,
                    b.power,
                    b.x,
                    b.y,
                    b.heading,
                    b.speed,
                )
            };
            if !b_alive || b_spawn >= tick {
                continue;
            }
            let h = b_heading.to_radians();
            let nx = b_x + b_speed * h.sin();
            let ny = b_y - b_speed * h.cos();

            // Find the first victim (robots in id order).
            let hit_r = cfg.tank_radius + cfg.bullet_pad;
            let victim: Option<usize> = self
                .robots
                .iter()
                .filter(|r| r.alive && r.id != b_owner)
                .find(|r| point_segment_dist(r.x, r.y, b_x, b_y, nx, ny) <= hit_r)
                .map(|r| r.id);

            if let Some(v) = victim {
                let damage = cfg.bullet_damage_factor * b_power;
                let (vx, vy) = (self.robots[v].x, self.robots[v].y);
                // Victim event.
                let mut ev = Event::empty(EventKind::HitByBullet);
                ev.x = b_x;
                ev.y = b_y;
                ev.bearing = bearing_deg(vx, vy, b_x, b_y);
                ev.dist = dist(vx, vy, b_x, b_y);
                ev.power = b_power;
                ev.name = self.robots[b_owner].name.clone();
                self.robots[v].queue_event(&cfg, ev);
                // Shooter event + energy return (only a live shooter
                // benefits; stats count either way).
                if self.robots[b_owner].alive {
                    let (ox, oy) = (self.robots[b_owner].x, self.robots[b_owner].y);
                    let mut ev = Event::empty(EventKind::BulletHit);
                    ev.x = vx;
                    ev.y = vy;
                    ev.bearing = bearing_deg(ox, oy, vx, vy);
                    ev.dist = dist(ox, oy, vx, vy);
                    ev.power = b_power;
                    ev.name = self.robots[v].name.clone();
                    self.robots[b_owner].queue_event(&cfg, ev);
                    self.robots[b_owner].energy += cfg.bullet_energy_return * b_power;
                }
                self.robots[b_owner].stats.bullet_hits += 1;
                self.robots[b_owner].stats.damage_dealt += damage;
                self.tick_log.push(format!(
                    "{}'s bullet hit {} for {:.1} damage",
                    self.robots[b_owner].name, self.robots[v].name, damage
                ));
                self.take_damage(v, damage);
                let b = &mut self.bullets[bi];
                b.x = vx;
                b.y = vy;
                b.alive = false;
            } else if nx < -cfg.tank_radius
                || nx > cfg.arena_w + cfg.tank_radius
                || ny < -cfg.tank_radius
                || ny > cfg.arena_h + cfg.tank_radius
            {
                if self.robots[b_owner].alive {
                    self.robots[b_owner].queue_event(&cfg, Event::empty(EventKind::BulletMissed));
                }
                let b = &mut self.bullets[bi];
                b.x = nx;
                b.y = ny;
                b.alive = false;
            } else {
                let b = &mut self.bullets[bi];
                b.x = nx;
                b.y = ny;
            }
        }
        self.bullets.retain(|b| b.alive);
    }

    // ----- Phase 4: deaths & end ----------------------------------------------

    fn death_phase(&mut self) {
        for i in 0..self.robots.len() {
            if self.robots[i].alive && self.robots[i].energy <= 0.0 {
                self.kill(i);
            }
        }
        let alive: Vec<usize> = (0..self.robots.len())
            .filter(|&i| self.robots[i].alive)
            .collect();
        if self.result.is_none() {
            match alive.len() {
                0 => self.finish(EndReason::LastStanding),
                1 => self.finish(EndReason::LastStanding),
                _ => {}
            }
        }
    }

    fn kill(&mut self, i: usize) {
        self.robots[i].alive = false;
        self.robots[i].pending = None;
        self.robots[i].vm = None;
        self.tick_log
            .push(format!("{} was destroyed", self.robots[i].name));
        let name = self.robots[i].name.clone();
        let (x, y) = (self.robots[i].x, self.robots[i].y);
        for j in 0..self.robots.len() {
            if j != i && self.robots[j].alive {
                let mut e = Event::empty(EventKind::RobotDeath);
                e.name = name.clone();
                e.x = x;
                e.y = y;
                self.robots[j].queue_event(&self.cfg, e);
            }
        }
    }

    fn finish(&mut self, reason: EndReason) {
        let winner = match reason {
            EndReason::LastStanding => (0..self.robots.len()).find(|&i| self.robots[i].alive),
            EndReason::TickLimit => {
                // Highest energy wins; exact ties are draws.
                let mut best: Option<usize> = None;
                let mut best_e = f64::NEG_INFINITY;
                let mut tie = false;
                for i in 0..self.robots.len() {
                    if !self.robots[i].alive {
                        continue;
                    }
                    if self.robots[i].energy > best_e {
                        best_e = self.robots[i].energy;
                        best = Some(i);
                        tie = false;
                    } else if self.robots[i].energy == best_e {
                        tie = true;
                    }
                }
                if tie {
                    None
                } else {
                    best
                }
            }
        };
        self.result = Some(BattleResult {
            winner,
            reason,
            tick: self.tick,
        });
    }

    // ----- Phase 5: radar -----------------------------------------------------

    fn radar_phase(&mut self) {
        let cfg = self.cfg.clone();
        let n = self.robots.len();
        for i in 0..n {
            if !self.robots[i].alive {
                continue;
            }
            let beam = cfg.radar_beam / 2.0;
            let (ix, iy) = (self.robots[i].x, self.robots[i].y);
            let cur = self.robots[i].radar_heading();
            let prev = self.robots[i].prev_radar_heading;
            let sweep = ang_diff(cur, prev);
            // Center and half-width of the union of the beam positions
            // throughout this tick's sweep.
            let mid = norm_deg(prev + sweep / 2.0);
            let half = sweep.abs() / 2.0 + beam;
            // Active radar locks the nearest robot inside the swept arc.
            let mut lock: Option<usize> = None;
            let mut lock_dist = f64::INFINITY;
            for j in 0..n {
                if j == i || !self.robots[j].alive {
                    continue;
                }
                let bearing = bearing_deg(ix, iy, self.robots[j].x, self.robots[j].y);
                if ang_diff(bearing, mid).abs() <= half {
                    let d = dist(ix, iy, self.robots[j].x, self.robots[j].y);
                    if d < lock_dist {
                        lock_dist = d;
                        lock = Some(j);
                    }
                }
            }
            if let Some(j) = lock {
                let t = &self.robots[j];
                let mut e = Event::empty(EventKind::Scanned);
                e.x = t.x;
                e.y = t.y;
                e.heading = t.body_heading;
                e.velocity = t.velocity;
                e.energy = t.energy;
                e.bearing = bearing_deg(ix, iy, t.x, t.y);
                e.dist = lock_dist;
                e.name = t.name.clone();
                self.robots[i].queue_event(&cfg, e);
            }
        }
    }

    // ----- Phase 6: snapshot --------------------------------------------------

    fn snapshot_phase(&mut self) {
        let robots: Vec<[f64; 7]> = self
            .robots
            .iter()
            .map(|r| {
                [
                    r.x,
                    r.y,
                    r.body_heading,
                    r.gun_heading(),
                    r.radar_heading(),
                    r.energy,
                    if r.alive { 1.0 } else { 0.0 },
                ]
            })
            .collect();
        let bullets: Vec<[f64; 4]> = self
            .bullets
            .iter()
            .filter(|b| b.alive)
            .map(|b| [b.x, b.y, b.owner as f64, b.power])
            .collect();
        self.snapshots.push(Snapshot {
            tick: self.tick,
            robots,
            bullets,
            events: self.tick_log.clone(),
        });
    }

    // ----- Damage -------------------------------------------------------------

    /// Apply damage and record stats. Deaths are resolved in death_phase.
    fn take_damage(&mut self, i: usize, amount: f64) {
        self.robots[i].energy -= amount;
        self.robots[i].stats.damage_taken += amount;
    }
}

/// Fastest speed at which a tank `distance` units from its goal can still
/// brake to a stop exactly there (Robocode's getMaxVelocity, generalized to
/// any deceleration).
fn move_speed(distance: f64, cfg: &Config) -> f64 {
    if distance <= 0.0 {
        return 0.0;
    }
    let decel = cfg.decel;
    // Ticks of braking needed from the speed we want to reach.
    let t = ((((8.0 / decel) * distance + 1.0).sqrt() - 1.0) / 2.0)
        .ceil()
        .max(1.0);
    let brake_dist = t / 2.0 * (t - 1.0) * decel;
    ((t - 1.0) * decel + (distance - brake_dist) / t).min(cfg.max_speed)
}

/// One tick of velocity change toward `target`: speeding up uses `accel`;
/// slowing down uses `decel`; reversing brakes to a stop first.
fn next_velocity(v: f64, target: f64, cfg: &Config) -> f64 {
    let same_dir = (v > 0.0) == (target > 0.0) && target != 0.0;
    if v == 0.0 || (same_dir && target.abs() >= v.abs()) {
        v + (target - v).clamp(-cfg.accel, cfg.accel)
    } else if target == 0.0 || same_dir {
        v + (target - v).clamp(-cfg.decel, cfg.decel)
    } else {
        v - v.signum() * v.abs().min(cfg.decel)
    }
}

fn pending_complete(p: &Pending, cfg: &Config) -> bool {
    match p {
        Pending::Move { remaining, .. } => *remaining <= cfg.epsilon,
        Pending::TurnBody { remaining }
        | Pending::TurnGun { remaining }
        | Pending::TurnRadar { remaining } => remaining.abs() <= cfg.epsilon,
        Pending::AwaitTick { n } => *n == 0,
    }
}

/// Random non-overlapping starting positions and headings, drawn from the
/// battle seed, so every battle with the same seed starts identically.
fn place_robots(cfg: &Config, robots: &mut [Robot], rng: &mut Rng) {
    let m = cfg.tank_radius * 3.0;
    let min_sep = cfg.tank_radius * 6.0;
    for i in 0..robots.len() {
        let mut placed = false;
        for _ in 0..200 {
            let x = rng.range(m, cfg.arena_w - m);
            let y = rng.range(m, cfg.arena_h - m);
            let ok = (0..i).all(|k| dist(robots[k].x, robots[k].y, x, y) >= min_sep);
            if ok {
                robots[i].x = x;
                robots[i].y = y;
                placed = true;
                break;
            }
        }
        if !placed {
            // Fallback: deterministic spread along a diagonal.
            let frac = i as f64 / (robots.len().max(2) - 1) as f64;
            robots[i].x = m + frac * (cfg.arena_w - 2.0 * m);
            robots[i].y = m + frac * (cfg.arena_h - 2.0 * m);
        }
        robots[i].body_heading = rng.range(0.0, 360.0);
        robots[i].prev_radar_heading = robots[i].body_heading;
    }
}
