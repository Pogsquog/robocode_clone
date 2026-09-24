//! tank-engine: the deterministic, headless battle engine.

pub mod battle;
pub mod config;
pub mod event;
pub mod host_impl;
pub mod replay;
pub mod rng;
pub mod robot;

pub use battle::{Battle, BattleResult, Bullet, EndReason, RobotSpec};
pub use config::Config;
pub use replay::write_replay;
pub use robot::{ang_diff, bearing_deg, dist, norm_deg};

#[cfg(test)]
mod tests {
    use super::*;

    const SWEEPER: &str = r#"
        func angdiff(a, b) {
            var d = (a - b) % 360;
            if (d > 180) { d = d - 360; }
            if (d <= -180) { d = d + 360; }
            return d;
        }
        func main() {
            set_radar_rate(45);
            while (true) {
                var e = pop_event();
                if (e != false) {
                    var diff = angdiff(event_bearing(), gun_heading());
                    set_gun_rate(diff * 2);
                    if (abs(diff) < 5 && gun_heat() <= 0) { fire(2); }
                }
                await_tick();
            }
        }
    "#;

    const CORNER: &str = r#"
        func angdiff(a, b) {
            var d = (a - b) % 360;
            if (d > 180) { d = d - 360; }
            if (d <= -180) { d = d + 360; }
            return d;
        }
        func main() {
            set_radar_rate(45);
            var tx = 60;
            var ty = 60;
            if (x() > arena_w() / 2) { tx = arena_w() - 60; }
            if (y() > arena_h() / 2) { ty = arena_h() - 60; }
            turn_body(angdiff(bearing_to(tx, ty), body_heading()));
            while (true) {
                var e = pop_event();
                if (e != false) {
                    var diff = angdiff(event_bearing(), gun_heading());
                    set_gun_rate(diff * 2);
                    if (abs(diff) < 5 && gun_heat() <= 0) { fire(1); }
                }
                await_tick();
            }
        }
    "#;

    fn make_battle(seed: u64) -> Battle {
        let cfg = Config::default();
        let specs = vec![
            RobotSpec {
                name: "sweeper".into(),
                source: SWEEPER.into(),
            },
            RobotSpec {
                name: "corner".into(),
                source: CORNER.into(),
            },
        ];
        Battle::new(cfg, &specs, seed, 3000).expect("battle")
    }

    #[test]
    fn battle_runs_to_completion() {
        let mut b = make_battle(42);
        b.run();
        assert!(b.result.is_some(), "battle must end");
        assert!(b.tick > 10, "battle should last a while, ended at {}", b.tick);
        let replay = write_replay(&b);
        assert!(replay.contains("\"format\":\"tank-replay\""));
        assert!(replay.contains("sweeper"));
    }

    #[test]
    fn determinism_same_seed_identical_replay() {
        let mut a = make_battle(1234);
        a.run();
        let mut b = make_battle(1234);
        b.run();
        assert_eq!(write_replay(&a), write_replay(&b));
        assert_eq!(a.tick, b.tick);
    }

    #[test]
    fn different_seed_different_battle() {
        let mut a = make_battle(1);
        a.run();
        let mut b = make_battle(2);
        b.run();
        // Different placements from the seed mean different replays.
        assert_ne!(write_replay(&a), write_replay(&b));
        assert_ne!(a.robots[0].x, b.robots[0].x);
    }

    #[test]
    fn battles_have_events_and_hits() {
        let mut b = make_battle(7);
        b.run();
        let hits: usize = b
            .snapshots
            .iter()
            .map(|s| s.events.iter().filter(|e| e.contains("bullet hit")).count())
            .sum();
        assert!(hits > 0, "aggressive bots should land shots");
        assert!(b.robots.iter().any(|r| r.stats.fired > 0));
    }

    // ----- Sandbox behavior ---------------------------------------------------

    fn sandbox_battle(bad_source: &str) -> Battle {
        let cfg = Config::default();
        let specs = vec![
            RobotSpec {
                name: "bad".into(),
                source: bad_source.into(),
            },
            RobotSpec {
                name: "good".into(),
                source: SWEEPER.into(),
            },
        ];
        Battle::new(cfg, &specs, 11, 3000).expect("battle")
    }

    #[test]
    fn runtime_fault_forfeits_only_the_offender() {
        let mut b = sandbox_battle("func main() { var x = 1 / 0; }");
        b.run();
        assert!(!b.robots[0].alive, "faulting robot must forfeit");
        assert!(b.robots[0].fault.as_deref().unwrap().contains("zero"));
        assert!(b.robots[1].alive, "the other robot must be unharmed");
        assert_eq!(b.result.as_ref().unwrap().winner, Some(1));
    }

    #[test]
    fn busy_loop_hits_budget_and_forfeits() {
        // No blocking call anywhere: burns the budget every tick.
        let mut b = sandbox_battle("func main() { while (true) { var x = 1 + 1; } }");
        b.run();
        assert!(!b.robots[0].alive, "budget-striking robot must forfeit");
        assert!(b
            .robots[0]
            .fault
            .as_deref()
            .unwrap()
            .contains("budget"));
        assert_eq!(b.result.as_ref().unwrap().winner, Some(1));
        // It must be disabled quickly, not stall the battle for long.
        assert!(b.tick < 100, "forfeited at tick {}", b.tick);
    }

    #[test]
    fn blocking_loop_is_the_intended_idiom_and_survives() {
        // await_tick inside the loop: one budget op per tick, never strikes.
        let mut b = sandbox_battle("func main() { while (true) { await_tick(); } }");
        for _ in 0..200 {
            if b.result.is_some() {
                break;
            }
            b.step();
        }
        assert_eq!(b.robots[0].stats.budget_strikes, 0);
        assert!(b.robots[0].alive, "a well-behaved robot must never forfeit");
        assert!(b.robots[0].pending.is_some(), "robot should be parked on await_tick");
    }

    #[test]
    fn halted_program_idles_gracefully() {
        let mut b = sandbox_battle("func main() { return; }");
        for _ in 0..50 {
            b.step();
        }
        assert!(b.robots[0].alive, "a halted robot idles; it does not die");
        assert!(b.robots[0].vm.is_none());
        assert!(b.result.is_none(), "battle continues without it ending early");
    }

    #[test]
    fn wall_collision_stops_and_damages() {
        // Drive into a wall and keep pushing: energy must drop.
        let mut b = sandbox_battle(
            "func main() { set_velocity(8); while (true) { await_tick(); } }",
        );
        let e0 = b.robots[0].energy;
        for _ in 0..300 {
            if b.result.is_some() {
                break;
            }
            b.step();
        }
        assert!(b.robots[0].energy < e0, "wall grinding must cost energy");
    }

}
