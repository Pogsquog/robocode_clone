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
        assert!(
            b.tick > 10,
            "battle should last a while, ended at {}",
            b.tick
        );
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
        assert!(b.robots[0].fault.as_deref().unwrap().contains("budget"));
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
        assert!(
            b.robots[0].pending.is_some(),
            "robot should be parked on await_tick"
        );
    }

    #[test]
    fn halted_program_idles_gracefully() {
        let mut b = sandbox_battle("func main() { return; }");
        for _ in 0..50 {
            b.step();
        }
        assert!(b.robots[0].alive, "a halted robot idles; it does not die");
        assert!(b.robots[0].vm.is_none());
        assert!(
            b.result.is_none(),
            "battle continues without it ending early"
        );
    }

    #[test]
    fn wall_collision_stops_and_damages() {
        // Drive into a wall and keep pushing: energy must drop.
        let mut b =
            sandbox_battle("func main() { set_velocity(8); while (true) { await_tick(); } }");
        let e0 = b.robots[0].energy;
        for _ in 0..300 {
            if b.result.is_some() {
                break;
            }
            b.step();
        }
        assert!(b.robots[0].energy < e0, "wall grinding must cost energy");
    }

    #[test]
    fn instantly_completing_blocking_calls_share_the_tick_budget() {
        // Each call completes at once and resumes the VM within the same
        // tick; without a shared budget this loop never yields.
        for call in [
            "ahead(0)",
            "back(0)",
            "turn_body(0)",
            "turn_gun(0)",
            "turn_radar(0)",
        ] {
            let src = format!("func main() {{ while (true) {{ {}; }} }}", call);
            let mut b = sandbox_battle(&src);
            b.run();
            assert!(!b.robots[0].alive, "{} loop must forfeit", call);
            assert!(b.robots[0].fault.as_deref().unwrap().contains("budget"));
            assert!(
                b.tick < 100,
                "{} loop forfeited late, at tick {}",
                call,
                b.tick
            );
        }
    }

    #[test]
    fn non_finite_numbers_cannot_enter_the_simulation() {
        for call in [
            "fire(sqrt(0 - 1))",
            "set_velocity(sqrt(0 - 1))",
            "set_body_rate(sqrt(0 - 1))",
            "turn_gun(sqrt(0 - 1))",
            "ahead(sqrt(0 - 1))",
            "bearing_to(sqrt(0 - 1), 0)",
        ] {
            let src = format!(
                "func main() {{ {}; while (true) {{ await_tick(); }} }}",
                call
            );
            let mut b = sandbox_battle(&src);
            b.run();
            assert!(!b.robots[0].alive, "{} must forfeit", call);
            assert!(
                b.robots[0].fault.as_deref().unwrap().contains("finite"),
                "{}: {:?}",
                call,
                b.robots[0].fault
            );
            // (The fault message itself says "NaN"; the numbers must not.)
            assert!(b.snapshots.iter().all(|s| s
                .robots
                .iter()
                .flatten()
                .chain(s.bullets.iter().flatten())
                .all(|v| v.is_finite())));
        }
    }

    #[test]
    fn string_doubling_cannot_exhaust_host_memory() {
        // Previously aborted the host with out-of-memory within ~30 ticks.
        let mut b = sandbox_battle(
            "func main() { var s = \"ab\"; while (true) { s = s + s; await_tick(); } }",
        );
        b.run();
        assert!(!b.robots[0].alive);
        assert!(b.robots[0]
            .fault
            .as_deref()
            .unwrap()
            .contains("string too long"));
        assert!(b.tick < 20, "forfeited at tick {}", b.tick);
    }

    #[test]
    fn nan_comparison_in_robot_code_is_harmless() {
        let mut b = sandbox_battle(
            "func main() { var n = sqrt(0 - 1); if (n < 1 || n >= 1) { fire(1); } \
             while (true) { await_tick(); } }",
        );
        for _ in 0..20 {
            b.step();
        }
        assert!(b.robots[0].alive, "{:?}", b.robots[0].fault);
        assert_eq!(b.robots[0].stats.fired, 0, "NaN comparisons are false");
    }

    /// Positions logged by a robot as "x,y" lines.
    fn logged_points(b: &Battle, id: usize) -> Vec<(f64, f64)> {
        b.logs
            .iter()
            .filter(|(_, r, _)| *r == id)
            .map(|(_, _, m)| {
                let (x, y) = m.split_once(',').unwrap();
                (x.parse().unwrap(), y.parse().unwrap())
            })
            .collect()
    }

    #[test]
    fn blocking_moves_cover_the_requested_distance() {
        let mut b = sandbox_battle(
            "func here() { log(x() + \",\" + y()); } \
             func main() { \
                here(); ahead(20); here(); \
                set_velocity(8); await_tick(); await_tick(); await_tick(); \
                set_velocity(0); here(); back(10); here(); \
                for (var i = 0; i < 5; i += 1) { await_tick(); } here(); \
                while (true) { await_tick(); } }",
        );
        for _ in 0..60 {
            b.step();
        }
        assert_eq!(b.robots[0].stats.damage_taken, 0.0, "must not hit a wall");
        let p = logged_points(&b, 0);
        assert_eq!(p.len(), 5);
        let d = |a: (f64, f64), b: (f64, f64)| dist(a.0, a.1, b.0, b.1);
        assert!(
            (d(p[0], p[1]) - 20.0).abs() < 1e-6,
            "ahead(20) moved {}",
            d(p[0], p[1])
        );
        // back(10) issued at speed: net displacement is still 10, backwards.
        assert!(
            (d(p[2], p[3]) - 10.0).abs() < 1e-6,
            "back(10) moved {}",
            d(p[2], p[3])
        );
        assert!(d(p[0], p[3]) < d(p[0], p[2]), "back() must move backwards");
        // And the tank stops where the move ended.
        assert!(
            d(p[3], p[4]) < 1e-6,
            "coasted {} after back()",
            d(p[3], p[4])
        );
    }

    #[test]
    fn a_move_stopped_by_a_wall_resumes_the_robot() {
        // Heads for a wall with a move far longer than the arena: the wall
        // must end the move and the code after it must run.
        let mut b = sandbox_battle(
            "func main() { ahead(5000); log(\"after\"); while (true) { await_tick(); } }",
        );
        for _ in 0..300 {
            b.step();
        }
        assert!(b.robots[0].alive, "{:?}", b.robots[0].fault);
        assert!(b.logs.iter().any(|(_, r, m)| *r == 0 && m == "after"));
    }

    #[test]
    fn a_move_stopped_by_a_collision_resumes_both_robots() {
        let specs: Vec<RobotSpec> = ["a", "b"]
            .iter()
            .map(|n| RobotSpec {
                name: n.to_string(),
                source:
                    "func main() { turn_body(norm_deg(bearing_to(500, 350) - body_heading())); \
                         ahead(5000); log(\"after\"); while (true) { await_tick(); } }"
                        .into(),
            })
            .collect();
        let mut b = Battle::new(Config::default(), &specs, 4, 3000).unwrap();
        for _ in 0..300 {
            b.step();
        }
        assert!(b
            .snapshots
            .iter()
            .any(|s| s.events.iter().any(|e| e.contains("collided"))));
        for id in 0..2 {
            assert!(b.robots[id].alive, "{:?}", b.robots[id].fault);
            assert!(b.logs.iter().any(|(_, r, m)| *r == id && m == "after"));
        }
    }

    #[test]
    fn atan2_inverts_sin_and_cos_in_degrees() {
        let mut b = sandbox_battle(
            "func main() { log(atan2(sin(30), cos(30))); log(atan2(sin(-120), cos(-120))); \
             log(atan2(1, 0)); log(norm_deg(atan2(8, 0 - 0))); log(atan2(0, 0)); \
             while (true) { await_tick(); } }",
        );
        b.step();
        let logs: Vec<&str> = b
            .logs
            .iter()
            .filter(|(_, r, _)| *r == 0)
            .map(|(_, _, m)| m.as_str())
            .collect();
        assert_eq!(logs, vec!["30", "-120", "90", "90", "0"]);
    }

    /// A battle against an opponent that never moves or fires.
    fn vs_sitting_duck(src: &str, seed: u64) -> Battle {
        let specs = vec![
            RobotSpec {
                name: "shooter".into(),
                source: src.into(),
            },
            RobotSpec {
                name: "duck".into(),
                source: "func main() { while (true) { await_tick(); } }".into(),
            },
        ];
        Battle::new(Config::default(), &specs, seed, 3000).unwrap()
    }

    #[test]
    fn fire_at_never_misses_a_stationary_target() {
        // Remember the target's bearing and request the shot every tick.
        let src = "func main() { var aim = -1; set_radar_rate(45); while (true) { \
                   if (pop_event() == \"scanned\") { aim = event_bearing(); } \
                   if (aim >= 0) { fire_at(aim, 1); } \
                   await_tick(); } }";
        for seed in 1..=5 {
            let mut b = vs_sitting_duck(src, seed);
            // Until the duck dies (after that, shots have nothing to hit).
            while b.result.is_none() && b.tick < 1000 {
                b.step();
            }
            let s = &b.robots[0].stats;
            assert!(s.fired >= 5, "seed {}: fired only {}", seed, s.fired);
            let in_flight = b.bullets.iter().filter(|x| x.owner == 0).count() as u32;
            assert_eq!(
                s.fired,
                s.bullet_hits + in_flight,
                "seed {}: every bullet must hit or still be flying",
                seed
            );
        }
    }

    #[test]
    fn fire_at_turns_at_most_the_gun_rate_and_fires_on_arrival() {
        let mut b = vs_sitting_duck(
            "func main() { var aim = gun_heading() + 90; \
             while (true) { fire_at(aim, 1); await_tick(); } }",
            7,
        );
        let start = b.robots[0].gun_heading();
        for tick in 1..=4 {
            b.step();
            let turned = ang_diff(b.robots[0].gun_heading(), start);
            assert!(
                (turned - 20.0 * tick as f64).abs() < 1e-9,
                "tick {}: {}",
                tick,
                turned
            );
            assert_eq!(b.robots[0].stats.fired, 0, "fired before reaching the aim");
        }
        b.step();
        assert!((ang_diff(b.robots[0].gun_heading(), start) - 90.0).abs() < 1e-9);
        assert_eq!(b.robots[0].stats.fired, 1, "fires the tick it arrives");
        let bullet = b.bullets.iter().find(|x| x.owner == 0).expect("bullet");
        assert!(ang_diff(bullet.heading, norm_deg(start + 90.0)).abs() < 1e-9);
        // Held on target while the gun cools: no second shot until it has.
        for _ in 0..9 {
            b.step();
        }
        assert_eq!(b.robots[0].stats.fired, 1);
    }

    #[test]
    fn fire_at_lasts_one_tick() {
        let mut b = vs_sitting_duck(
            "func main() { fire_at(gun_heading() + 90, 1); while (true) { await_tick(); } }",
            7,
        );
        let start = b.robots[0].gun_heading();
        for _ in 0..5 {
            b.step();
        }
        assert!((ang_diff(b.robots[0].gun_heading(), start) - 20.0).abs() < 1e-9);
        assert_eq!(b.robots[0].stats.fired, 0);
    }

    #[test]
    fn log_cap_is_per_robot() {
        let cfg = Config::default();
        let specs = vec![
            RobotSpec {
                name: "spammer".into(),
                source: "func main() { while (true) { \
                         for (var i = 0; i < 50; i += 1) { log(\"spam\"); } await_tick(); } }"
                    .into(),
            },
            RobotSpec {
                name: "quiet".into(),
                source: "func main() { while (true) { log(\"hi\"); await_tick(); } }".into(),
            },
        ];
        let mut b = Battle::new(cfg.clone(), &specs, 3, 3000).unwrap();
        for _ in 0..100 {
            b.step();
        }
        let count = |id| b.logs.iter().filter(|(_, r, _)| *r == id).count();
        assert_eq!(count(0), cfg.max_logs);
        assert_eq!(
            count(1),
            100,
            "the spammer must not use up others' log quota"
        );
    }

    #[test]
    fn dead_shooter_gets_no_refund_and_spent_bullets_are_dropped() {
        let mut b = sandbox_battle("func main() { while (true) { await_tick(); } }");
        b.step();
        b.robots[0].alive = false;
        let (x, y) = (b.robots[1].x, b.robots[1].y);
        b.bullets.push(Bullet {
            owner: 0,
            x,
            y: y + 30.0,
            heading: 0.0,
            speed: 12.0,
            power: 2.0,
            spawn_tick: 0,
            alive: true,
        });
        let (e0, e1) = (b.robots[0].energy, b.robots[1].energy);
        b.step();
        assert!(b.robots[1].energy < e1, "the bullet must hit");
        assert_eq!(b.robots[0].energy, e0, "a dead shooter gains nothing");
        assert!(b.robots[0].events.is_empty());
        assert!(b.bullets.is_empty(), "spent bullets are removed");
    }
}
