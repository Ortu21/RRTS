//! All industrial tuning. Work is throughput, never a stored currency.
use crate::units::UnitKind;
use bevy::prelude::*;

pub const TICK_SECONDS: f64 = 0.05;
pub const BASE_POWER: f64 = 10.0;
pub const FACTORY_POWER: f64 = 10.0;
/// Build radius around a live Commander: work flows only from builders
/// within their own radius of the site (see economy::site_power).
pub const COMMAND_BUILD_RADIUS: f32 = 26.0;
/// Build power granted by the Commander loop (same scale as base work rate
/// for now; per-builder power is a future upgrade hook).
pub const COMMANDER_BUILD_POWER: f64 = 10.0;
/// Mobile builder produced by the laboratory: half the throughput, smaller
/// radius. Stacks with the Commander when both are alive.
pub const ENGINEER_BUILD_POWER: f64 = 5.0;
pub const ENGINEER_BUILD_RADIUS: f32 = 16.0;
/// Sight (vision) ranges for fog of war: unarmed builders spot less than
/// troops, buildings watch their surroundings. Troops see as far as they
/// acquire (see sight_range).
pub const ENGINEER_SIGHT: f32 = 22.0;
pub const BUILDING_SIGHT: f32 = 26.0;
pub const STORAGE: [f64; 2] = [1000.0, 1500.0];
pub const INITIAL_STOCK: [f64; 2] = [400.0, 220.0];
pub const MAX_QUEUE: usize = 12;

#[derive(Clone, Copy, Debug)]
pub struct Cost {
    pub resources: [f64; 2],
    pub work: f64,
}
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingKind {
    Metal,
    Solar,
    Factory,
}
impl BuildingKind {
    pub const ALL: [Self; 3] = [Self::Metal, Self::Solar, Self::Factory];
    pub fn stats(self) -> &'static BuildingStats {
        &BUILDINGS[self as usize]
    }
}
pub struct BuildingStats {
    pub name: &'static str,
    pub cost: Cost,
    pub income: [f64; 2],
    pub health: f32,
    pub half: Vec2,
    pub height: f32,
}
pub const BUILDINGS: [BuildingStats; 3] = [
    BuildingStats {
        name: "Metal generator",
        cost: Cost {
            resources: [100.0, 80.0],
            work: 100.0,
        },
        income: [5.0, 0.0],
        health: 350.0,
        half: Vec2::new(2.5, 2.5),
        height: 3.0,
    },
    BuildingStats {
        name: "Solar panel",
        cost: Cost {
            resources: [80.0, 0.0],
            work: 80.0,
        },
        income: [0.0, 12.0],
        health: 250.0,
        half: Vec2::new(3.0, 2.0),
        height: 1.8,
    },
    BuildingStats {
        name: "Vehicle factory",
        cost: Cost {
            resources: [260.0, 240.0],
            work: 200.0,
        },
        income: [0.0, 0.0],
        health: 800.0,
        half: Vec2::new(5.0, 4.0),
        height: 4.0,
    },
];
pub const UNIT_COSTS: [Cost; 5] = [
    Cost {
        resources: [45.0, 100.0],
        work: 60.0,
    },
    Cost {
        resources: [90.0, 220.0],
        work: 100.0,
    },
    Cost {
        resources: [140.0, 360.0],
        work: 150.0,
    },
    // Commander is never queued in factories (see UnitKind::PRODUCIBLE);
    // placeholder keeps unit_cost() total over ALL without panicking.
    Cost {
        resources: [0.0, 0.0],
        work: 1.0,
    },
    // Engineer: cheap mobile builder from the laboratory.
    Cost {
        resources: [70.0, 50.0],
        work: 80.0,
    },
];
pub fn unit_cost(kind: UnitKind) -> Cost {
    UNIT_COSTS[kind.index()]
}
