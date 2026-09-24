//! The host interface: the full set of engine-provided functions callable
//! from robot code, plus the `Host` trait the engine implements.
//!
//! This is the sandbox boundary: robot bytecode can only interact with the
//! engine through the functions enumerated here, so a robot can never
//! perform I/O, allocate unbounded memory, or stall a battle.

use crate::value::Value;

/// Engine-provided functions. The discriminant is the bytecode-level index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum HostFn {
    // Blocking motion (VM suspends until complete).
    Ahead,
    Back,
    TurnBody,
    TurnGun,
    TurnRadar,
    AwaitTick,
    // Immediate intent setters (persist until changed).
    SetVelocity,
    SetBodyRate,
    SetGunRate,
    SetRadarRate,
    // Actions.
    Fire,
    Log,
    // State getters.
    X,
    Y,
    Velocity,
    Energy,
    BodyHeading,
    GunHeading,
    RadarHeading,
    GunHeat,
    Time,
    ArenaW,
    ArenaH,
    // Event queue.
    PopEvent,
    EventX,
    EventY,
    EventHeading,
    EventVelocity,
    EventEnergy,
    EventBearing,
    EventDist,
    EventPower,
    EventName,
    // Math helpers.
    Sin,
    Cos,
    Abs,
    Min,
    Max,
    Sqrt,
    NormDeg,
    BearingTo,
    Atan2,
}

/// (function, name, arity). Order must match the enum discriminants.
pub const HOST_FN_TABLE: &[(HostFn, &str, u8)] = &[
    (HostFn::Ahead, "ahead", 1),
    (HostFn::Back, "back", 1),
    (HostFn::TurnBody, "turn_body", 1),
    (HostFn::TurnGun, "turn_gun", 1),
    (HostFn::TurnRadar, "turn_radar", 1),
    (HostFn::AwaitTick, "await_tick", 0),
    (HostFn::SetVelocity, "set_velocity", 1),
    (HostFn::SetBodyRate, "set_body_rate", 1),
    (HostFn::SetGunRate, "set_gun_rate", 1),
    (HostFn::SetRadarRate, "set_radar_rate", 1),
    (HostFn::Fire, "fire", 1),
    (HostFn::Log, "log", 1),
    (HostFn::X, "x", 0),
    (HostFn::Y, "y", 0),
    (HostFn::Velocity, "velocity", 0),
    (HostFn::Energy, "energy", 0),
    (HostFn::BodyHeading, "body_heading", 0),
    (HostFn::GunHeading, "gun_heading", 0),
    (HostFn::RadarHeading, "radar_heading", 0),
    (HostFn::GunHeat, "gun_heat", 0),
    (HostFn::Time, "time", 0),
    (HostFn::ArenaW, "arena_w", 0),
    (HostFn::ArenaH, "arena_h", 0),
    (HostFn::PopEvent, "pop_event", 0),
    (HostFn::EventX, "event_x", 0),
    (HostFn::EventY, "event_y", 0),
    (HostFn::EventHeading, "event_heading", 0),
    (HostFn::EventVelocity, "event_velocity", 0),
    (HostFn::EventEnergy, "event_energy", 0),
    (HostFn::EventBearing, "event_bearing", 0),
    (HostFn::EventDist, "event_dist", 0),
    (HostFn::EventPower, "event_power", 0),
    (HostFn::EventName, "event_name", 0),
    (HostFn::Sin, "sin", 1),
    (HostFn::Cos, "cos", 1),
    (HostFn::Abs, "abs", 1),
    (HostFn::Min, "min", 2),
    (HostFn::Max, "max", 2),
    (HostFn::Sqrt, "sqrt", 1),
    (HostFn::NormDeg, "norm_deg", 1),
    (HostFn::BearingTo, "bearing_to", 2),
    (HostFn::Atan2, "atan2", 2),
];

impl HostFn {
    pub fn from_index(index: u16) -> Option<HostFn> {
        HOST_FN_TABLE.get(index as usize).map(|(f, _, _)| *f)
    }

    pub fn index(self) -> u16 {
        HOST_FN_TABLE
            .iter()
            .position(|(f, _, _)| *f == self)
            .expect("host fn present in table") as u16
    }
}

/// Lookup a host function by its source-level name.
pub fn host_lookup(name: &str) -> Option<(HostFn, u8)> {
    HOST_FN_TABLE
        .iter()
        .find(|(_, n, _)| *n == name)
        .map(|(f, _, a)| (*f, *a))
}

/// A request from robot code to suspend until the engine finishes something.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlockRequest {
    Ahead(f64),
    Back(f64),
    TurnBody(f64),
    TurnGun(f64),
    TurnRadar(f64),
    AwaitTick,
}

/// Result of a host call: either a value right now, or suspension.
pub enum HostOutcome {
    Value(Value),
    Block(BlockRequest),
}

/// The engine side of the sandbox boundary.
pub trait Host {
    /// `Err` is a runtime fault: the offending robot forfeits the battle.
    fn call(&mut self, f: HostFn, args: Vec<Value>) -> Result<HostOutcome, String>;
}
