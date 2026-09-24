//! Engine implementation of the robolang `Host` trait: the only way robot
//! code can touch the simulation.

use crate::battle::Battle;
use crate::robot::{bearing_deg, norm_deg};
use robolang::{Host, HostFn, HostOutcome, Value};

pub struct RobotHost<'a> {
    battle: &'a mut Battle,
    /// The robot whose VM is calling.
    id: usize,
}

/// Fetch a numeric argument. Non-finite values (NaN, infinity) are a fault:
/// they would otherwise leak into the simulation and break its invariants
/// (a NaN energy never reaches zero; a NaN position is never hit).
fn num_arg(args: &[Value], i: usize, what: &str) -> Result<f64, String> {
    let n = args
        .get(i)
        .and_then(|v| v.as_num())
        .ok_or_else(|| format!("'{}' expects a number for argument {}", what, i + 1))?;
    if !n.is_finite() {
        return Err(format!(
            "'{}' got {} for argument {}; numbers passed to the engine must be finite",
            what,
            Value::Num(n),
            i + 1
        ));
    }
    Ok(n)
}

impl<'a> RobotHost<'a> {
    pub fn new(battle: &'a mut Battle, id: usize) -> RobotHost<'a> {
        RobotHost { battle, id }
    }
}

impl<'a> Host for RobotHost<'a> {
    fn call(&mut self, f: HostFn, args: Vec<Value>) -> Result<HostOutcome, String> {
        use HostFn::*;
        let id = self.id;

        // Blocking motion and firing are handled without holding robot borrows.
        match f {
            Ahead => {
                return Ok(HostOutcome::Block(robolang::BlockRequest::Ahead(num_arg(
                    &args, 0, "ahead",
                )?)))
            }
            Back => {
                return Ok(HostOutcome::Block(robolang::BlockRequest::Back(num_arg(
                    &args, 0, "back",
                )?)))
            }
            TurnBody => {
                return Ok(HostOutcome::Block(robolang::BlockRequest::TurnBody(
                    num_arg(&args, 0, "turn_body")?,
                )))
            }
            TurnGun => {
                return Ok(HostOutcome::Block(robolang::BlockRequest::TurnGun(
                    num_arg(&args, 0, "turn_gun")?,
                )))
            }
            TurnRadar => {
                return Ok(HostOutcome::Block(robolang::BlockRequest::TurnRadar(
                    num_arg(&args, 0, "turn_radar")?,
                )))
            }
            AwaitTick => return Ok(HostOutcome::Block(robolang::BlockRequest::AwaitTick)),
            Fire => {
                let power = num_arg(&args, 0, "fire")?;
                return Ok(HostOutcome::Value(Value::Bool(
                    self.battle.try_fire(id, power),
                )));
            }
            _ => {}
        }

        match f {
            // Immediate intent setters.
            SetVelocity => {
                let v = num_arg(&args, 0, "set_velocity")?;
                let max = self.battle.cfg.max_speed;
                let r = &mut self.battle.robots[id];
                r.intent_velocity = v.clamp(-max, max);
                Ok(HostOutcome::Value(Value::Null))
            }
            SetBodyRate => {
                let v = num_arg(&args, 0, "set_body_rate")?;
                let max = self.battle.cfg.max_body_rate;
                let r = &mut self.battle.robots[id];
                r.intent_body_rate = v.clamp(-max, max);
                Ok(HostOutcome::Value(Value::Null))
            }
            SetGunRate => {
                let v = num_arg(&args, 0, "set_gun_rate")?;
                let max = self.battle.cfg.max_gun_rate;
                let r = &mut self.battle.robots[id];
                r.intent_gun_rate = v.clamp(-max, max);
                Ok(HostOutcome::Value(Value::Null))
            }
            SetRadarRate => {
                let v = num_arg(&args, 0, "set_radar_rate")?;
                let max = self.battle.cfg.max_radar_rate;
                let r = &mut self.battle.robots[id];
                r.intent_radar_rate = v.clamp(-max, max);
                Ok(HostOutcome::Value(Value::Null))
            }

            // Actions.
            FireAt => {
                // Resolved at the end of the movement phase (Battle::
                // resolve_fire_requests): turn the gun onto `heading`, and
                // fire along it if the gun got there and can fire.
                let heading = norm_deg(num_arg(&args, 0, "fire_at")?);
                let power = num_arg(&args, 1, "fire_at")?;
                self.battle.robots[id].fire_request = Some((heading, power));
                Ok(HostOutcome::Value(Value::Null))
            }
            Log => {
                let msg = args.first().map(|v| v.to_display()).unwrap_or_default();
                if self.battle.robots[id].stats.logs < self.battle.cfg.max_logs {
                    self.battle.robots[id].stats.logs += 1;
                    let name = self.battle.robots[id].name.clone();
                    let tick = self.battle.tick;
                    self.battle.logs.push((tick, id, msg.clone()));
                    self.battle.tick_log.push(format!("{}: {}", name, msg));
                }
                Ok(HostOutcome::Value(Value::Null))
            }

            // State getters.
            X => Ok(HostOutcome::Value(Value::Num(self.battle.robots[id].x))),
            Y => Ok(HostOutcome::Value(Value::Num(self.battle.robots[id].y))),
            Velocity => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id].velocity,
            ))),
            Energy => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id].energy,
            ))),
            BodyHeading => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id].body_heading,
            ))),
            GunHeading => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id].gun_heading(),
            ))),
            RadarHeading => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id].radar_heading(),
            ))),
            GunHeat => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id].gun_heat,
            ))),
            Time => Ok(HostOutcome::Value(Value::Num(self.battle.tick as f64))),
            ArenaW => Ok(HostOutcome::Value(Value::Num(self.battle.cfg.arena_w))),
            ArenaH => Ok(HostOutcome::Value(Value::Num(self.battle.cfg.arena_h))),

            // Event queue.
            PopEvent => {
                let r = &mut self.battle.robots[id];
                match r.events.pop_front() {
                    Some(e) => {
                        let kind = e.kind.as_str().to_string();
                        r.current_event = Some(e);
                        Ok(HostOutcome::Value(Value::Str(kind)))
                    }
                    None => Ok(HostOutcome::Value(Value::Bool(false))),
                }
            }
            EventX => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id]
                    .current_event
                    .as_ref()
                    .map(|e| e.x)
                    .unwrap_or(0.0),
            ))),
            EventY => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id]
                    .current_event
                    .as_ref()
                    .map(|e| e.y)
                    .unwrap_or(0.0),
            ))),
            EventHeading => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id]
                    .current_event
                    .as_ref()
                    .map(|e| e.heading)
                    .unwrap_or(0.0),
            ))),
            EventVelocity => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id]
                    .current_event
                    .as_ref()
                    .map(|e| e.velocity)
                    .unwrap_or(0.0),
            ))),
            EventEnergy => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id]
                    .current_event
                    .as_ref()
                    .map(|e| e.energy)
                    .unwrap_or(0.0),
            ))),
            EventBearing => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id]
                    .current_event
                    .as_ref()
                    .map(|e| e.bearing)
                    .unwrap_or(0.0),
            ))),
            EventDist => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id]
                    .current_event
                    .as_ref()
                    .map(|e| e.dist)
                    .unwrap_or(0.0),
            ))),
            EventPower => Ok(HostOutcome::Value(Value::Num(
                self.battle.robots[id]
                    .current_event
                    .as_ref()
                    .map(|e| e.power)
                    .unwrap_or(0.0),
            ))),
            EventName => Ok(HostOutcome::Value(Value::Str(
                self.battle.robots[id]
                    .current_event
                    .as_ref()
                    .map(|e| e.name.clone())
                    .unwrap_or_default(),
            ))),

            // Math helpers.
            Sin => Ok(HostOutcome::Value(Value::Num(
                num_arg(&args, 0, "sin")?.to_radians().sin(),
            ))),
            Cos => Ok(HostOutcome::Value(Value::Num(
                num_arg(&args, 0, "cos")?.to_radians().cos(),
            ))),
            Abs => Ok(HostOutcome::Value(Value::Num(
                num_arg(&args, 0, "abs")?.abs(),
            ))),
            Min => Ok(HostOutcome::Value(Value::Num(
                num_arg(&args, 0, "min")?.min(num_arg(&args, 1, "min")?),
            ))),
            Max => Ok(HostOutcome::Value(Value::Num(
                num_arg(&args, 0, "max")?.max(num_arg(&args, 1, "max")?),
            ))),
            Sqrt => Ok(HostOutcome::Value(Value::Num(
                num_arg(&args, 0, "sqrt")?.sqrt(),
            ))),
            NormDeg => Ok(HostOutcome::Value(Value::Num(norm_deg(num_arg(
                &args, 0, "norm_deg",
            )?)))),
            BearingTo => {
                let tx = num_arg(&args, 0, "bearing_to")?;
                let ty = num_arg(&args, 1, "bearing_to")?;
                let r = &self.battle.robots[id];
                Ok(HostOutcome::Value(Value::Num(bearing_deg(
                    r.x, r.y, tx, ty,
                ))))
            }
            // Plain maths, in degrees: the inverse of sin/cos, so
            // atan2(sin(a), cos(a)) == a (for a in -180..180).
            Atan2 => Ok(HostOutcome::Value(Value::Num(
                num_arg(&args, 0, "atan2")?
                    .atan2(num_arg(&args, 1, "atan2")?)
                    .to_degrees(),
            ))),

            // Handled above; unreachable here.
            Ahead | Back | TurnBody | TurnGun | TurnRadar | AwaitTick | Fire => {
                unreachable!("blocking ops handled early")
            }
        }
    }
}
