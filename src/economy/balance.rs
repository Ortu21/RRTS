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
    Turret,
    Wall,
    LabT2,
}
impl BuildingKind {
    pub const ALL: [Self; 6] = [
        Self::Metal,
        Self::Solar,
        Self::Factory,
        Self::Turret,
        Self::Wall,
        Self::LabT2,
    ];
    pub fn stats(self) -> &'static BuildingStats {
        &BUILDINGS[self as usize]
    }
    /// Static defenses never act as builders or factories.
    /// Phase-4 hook: the AI counts these to plan base defense.
    #[allow(dead_code)]
    pub fn is_defense(self) -> bool {
        matches!(self, Self::Turret | Self::Wall)
    }
    /// Produces units (tier-gated): the T1 lab and the T2 lab.
    pub fn is_factory(self) -> bool {
        matches!(self, Self::Factory | Self::LabT2)
    }
}
pub struct BuildingStats {
    pub name: &'static str,
    pub cost: Cost,
    pub income: [f64; 2],
    pub health: f32,
    pub half: Vec2,
    pub height: f32,
    /// Fog sight contributed while alive. Walls are blind (0.0).
    pub sight: f32,
}
pub const BUILDINGS: [BuildingStats; 6] = [
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
        sight: 26.0,
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
        sight: 26.0,
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
        sight: 26.0,
    },
    BuildingStats {
        name: "Laser turret",
        cost: Cost {
            resources: [150.0, 150.0],
            work: 120.0,
        },
        income: [0.0, 0.0],
        health: 450.0,
        half: Vec2::new(2.0, 2.0),
        height: 2.5,
        sight: 24.0,
    },
    BuildingStats {
        name: "Wall",
        cost: Cost {
            resources: [20.0, 0.0],
            work: 20.0,
        },
        income: [0.0, 0.0],
        health: 700.0,
        half: Vec2::new(1.0, 1.0),
        height: 1.5,
        sight: 0.0,
    },
    BuildingStats {
        name: "Tier-2 laboratory",
        cost: Cost {
            resources: [300.0, 300.0],
            work: 250.0,
        },
        income: [0.0, 0.0],
        health: 900.0,
        half: Vec2::new(6.0, 5.0),
        height: 4.5,
        sight: 26.0,
    },
];
pub const UNIT_COSTS: [Cost; 8] = [
    Cost {
        resources: [45.0, 100.0],
        work: 60.0,
    },
    Cost {
        resources: [110.0, 260.0],
        work: 120.0,
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
    // Light tank: cheap MG screening unit.
    Cost {
        resources: [60.0, 140.0],
        work: 80.0,
    },
    // Tier-2 heavies: same roles, bigger hulls. Need a LabT2 (see Factory.tier).
    Cost {
        resources: [220.0, 520.0],
        work: 240.0,
    },
    Cost {
        resources: [280.0, 720.0],
        work: 300.0,
    },
];
pub fn unit_cost(kind: UnitKind) -> Cost {
    UNIT_COSTS[kind.index()]
}

/// Static defense gun table (base decente per future torrette: nuovi tipi =
/// nuove righe + match in `turret_stats`, mai branch nei sistemi combat).
/// Oggi esiste solo il laser; contraerea/surriscaldamento/upgrade agganciano qui.
#[derive(Debug, Clone, Copy)]
pub struct TurretStats {
    pub range: f32,
    pub cooldown: f32,
    pub damage: f32,
    pub projectile_speed: f32,
    pub acquisition: f32,
    pub traverse: f32,
    pub aim_tolerance: f32,
}

pub const LASER_TURRET: TurretStats = TurretStats {
    range: 24.0,
    cooldown: 0.8,
    damage: 12.0,
    projectile_speed: 40.0,
    acquisition: 30.0,
    traverse: 3.5,
    aim_tolerance: 0.12,
};

pub fn turret_stats(kind: BuildingKind) -> Option<&'static TurretStats> {
    match kind {
        BuildingKind::Turret => Some(&LASER_TURRET),
        _ => None,
    }
}

/// Counter matrix (0.0.17): moltiplicatore di riga-kind vs colonna-kind.
/// Dato, non logica: i sistemi leggono tramite `counter_mult`, mai branch.
/// Ordine righe/colonne = `UnitKind` (Scout, Heavy, Arty, Commander, Engineer,
/// Light, Heavy2, Arty2). Quasi tutto 1.0; solo celle motivate dalle tabelle:
/// Heavy vince le risse coi Light (alpha/armatura), i Light chiudono sulle
/// Arty (10 vs 4.5 speed) che faticano contro bersagli veloci vicini.
pub const COUNTER_TABLE: [[f32; 8]; 8] = [
    //                 Scout  Heavy  Arty   Cmdr   Eng    Light  Hvy2   Arty2
    /* Scout      */
    [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
    /* HeavyTank  */ [1.0, 1.0, 1.0, 1.0, 1.0, 1.15, 1.0, 1.0],
    /* Artillery  */ [1.0, 1.0, 1.0, 1.0, 1.0, 0.85, 1.0, 1.0],
    /* Commander  */ [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
    /* Engineer   */ [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
    /* LightTank  */ [1.0, 0.9, 1.25, 1.0, 1.0, 1.0, 0.9, 1.25],
    /* HeavyTank2 */ [1.0, 1.0, 1.0, 1.0, 1.0, 1.15, 1.0, 1.0],
    /* Artillery2 */ [1.0, 1.0, 1.0, 1.0, 1.0, 0.85, 1.0, 1.0],
];

/// Lookup counter da tabella: mai branch per-kind fuori da qui.
pub fn counter_mult(atk: UnitKind, def: UnitKind) -> f32 {
    COUNTER_TABLE[atk.index()][def.index()]
}
