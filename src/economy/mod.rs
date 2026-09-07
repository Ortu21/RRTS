//! Fixed-step, team-isolated streaming economy shared by construction and factories.
pub mod balance;
pub mod benchmark;
#[cfg(test)]
pub(crate) mod tests;
use crate::{
    combat::Health,
    orders::UnitOrder,
    production::Factory,
    structures::{Building, Construction},
    units::{Builder, Team, Unit},
};
use balance::*;
use bevy::prelude::*;
use std::collections::BTreeMap;

pub struct EconomyPlugin;
impl Plugin for EconomyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Economy>()
            .insert_resource(Time::<Fixed>::from_seconds(TICK_SECONDS))
            .add_systems(FixedUpdate, tick.in_set(EconomyTick));
    }
}
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct EconomyTick;
#[derive(Resource, Default)]
pub struct Economy(pub BTreeMap<u8, Account>);
#[derive(Clone, Debug)]
pub struct Account {
    pub stock: [f64; 2],
    pub capacity: [f64; 2],
    pub income: [f64; 2],
    pub consumption: [f64; 2],
    pub demand: [f64; 2],
}
impl Default for Account {
    fn default() -> Self {
        Self {
            stock: INITIAL_STOCK,
            capacity: STORAGE,
            income: [0.0; 2],
            consumption: [0.0; 2],
            demand: [0.0; 2],
        }
    }
}
#[derive(Clone, Debug)]
pub struct Project {
    pub cost: Cost,
    pub done: f64,
    pub spent: [f64; 2],
    pub speed: f64,
    pub shortage: [bool; 2],
    /// Power applied on the last tick (sum of in-range builders). 0 with an
    /// unfinished site means no builder is close enough yet.
    pub power: f64,
}
impl Project {
    pub fn new(cost: Cost) -> Self {
        Self {
            cost,
            done: 0.0,
            spent: [0.0; 2],
            speed: 0.0,
            shortage: [false; 2],
            power: 0.0,
        }
    }
    pub fn complete(&self) -> bool {
        self.done >= self.cost.work
    }
    pub fn fraction(&self) -> f64 {
        self.done / self.cost.work
    }
    pub fn status(&self) -> &'static str {
        match (self.shortage[0], self.shortage[1]) {
            (true, true) => "SHORTAGE: metal + energy",
            (true, false) => "SHORTAGE: metal",
            (false, true) => "SHORTAGE: energy",
            _ if self.complete() => "Complete",
            _ if self.power <= 0.0 => "WAITING FOR BUILDER: move one in range",
            _ => "Working",
        }
    }
}

/// Max-min fair satisfaction of each consumer's requested work this tick.
/// Grow all fractions equally, freeze those needing an exhausted resource,
/// and continue with independent consumers (e.g. metal-only solar recovery).
/// Stable request ordering makes floating-point summation independent of ECS order.
pub fn allocate(stock: [f64; 2], demands: &[[f64; 2]]) -> Vec<f64> {
    let mut available = stock;
    let mut fractions = vec![0.0; demands.len()];
    let mut active = vec![true; demands.len()];
    for _ in 0..=demands.len() {
        let mut total = [0.0; 2];
        let mut step: f64 = 1.0;
        let mut any = false;
        for (i, demand) in demands.iter().enumerate() {
            if active[i] {
                any = true;
                step = step.min(1.0 - fractions[i]);
                for r in 0..2 {
                    total[r] += demand[r];
                }
            }
        }
        if !any {
            break;
        }
        for r in 0..2 {
            if total[r] > 0.0 {
                step = step.min(available[r] / total[r]);
            }
        }
        step = step.max(0.0);
        for (i, demand) in demands.iter().enumerate() {
            if active[i] {
                fractions[i] += step;
                for r in 0..2 {
                    available[r] = (available[r] - demand[r] * step).max(0.0);
                }
            }
        }
        for (i, demand) in demands.iter().enumerate() {
            if fractions[i] >= 1.0 - 1e-12
                || (0..2).any(|r| total[r] > 0.0 && available[r] <= 1e-10 && demand[r] > 0.0)
            {
                active[i] = false;
            }
        }
    }
    fractions
}

/// Build power for a site: only builders explicitly tasked on it contribute —
/// standing in range is not enough. Direct: alive same-team builder with
/// `Build { site }` inside its own radius. Assist: alive same-team builder
/// guarding (`Guard { ward }`) a direct builder of this site, also in range.
/// `any_builders` is false only in worlds that never spawned builders (unit
/// tests, benchmarks): there the legacy BASE_POWER applies. Anywhere else a
/// site with nobody tasked stalls at 0 — moving a builder away (any order
/// replaces Build) pauses it, walking one back resumes it.
pub fn site_power(
    site: Entity,
    site_pos: Vec3,
    team: u8,
    builders: &[(Entity, u8, Vec3, f64, f32, UnitOrder)],
    any_builders: bool,
) -> f64 {
    if !any_builders {
        return BASE_POWER;
    }
    let direct: Vec<Entity> = builders
        .iter()
        .filter(|(_, t, _, _, _, order)| {
            *t == team && matches!(order, UnitOrder::Build { site: s } if *s == site)
        })
        .map(|(e, _, _, _, _, _)| *e)
        .collect();
    builders
        .iter()
        .filter(|(_, t, pos, _, radius, _)| {
            *t == team && pos.xz().distance(site_pos.xz()) <= *radius
        })
        .filter(|(e, _, _, _, _, order)| match order {
            UnitOrder::Build { site: s } => *s == site,
            UnitOrder::Guard { target: ward } => direct.contains(ward) && *e != *ward,
            _ => false,
        })
        .map(|(_, _, _, power, _, _)| *power)
        .sum()
}
struct Request {
    entity: Entity,
    construction: bool,
    work: f64,
    demand: [f64; 2],
}
#[allow(clippy::type_complexity)]
fn tick(
    mut economy: ResMut<Economy>,
    buildings: Query<
        (
            Entity,
            &Team,
            &BuildingKind,
            &Transform,
            &Health,
            Has<Construction>,
        ),
        With<Building>,
    >,
    mut projects: Query<&mut Construction>,
    mut factories: Query<&mut Factory>,
    builders: Query<(Entity, &Team, &Transform, &Builder, &Health, &UnitOrder), With<Unit>>,
) {
    for account in economy.0.values_mut() {
        account.income = [0.0; 2];
        account.demand = [0.0; 2];
        account.consumption = [0.0; 2];
    }
    // Builders as flat data, deterministic order: proximity power stays
    // bit-for-bit stable across repeats and independent of ECS chunk order.
    // Orders ride along: only tasked builders (Build / Guard-assist) count.
    let mut sorted_builders: Vec<_> = builders.iter().collect();
    sorted_builders.sort_by_key(|row| row.0.to_bits());
    let flat: Vec<(Entity, u8, Vec3, f64, f32, UnitOrder)> = sorted_builders
        .iter()
        .filter(|(_, _, _, _, health, _)| health.current > 0.0)
        .map(|(e, team, transform, builder, _, order)| {
            (
                *e,
                team.0,
                transform.translation,
                builder.power,
                builder.radius,
                (*order).clone(),
            )
        })
        .collect();
    let any_builders = !builders.is_empty();
    let mut by_team: BTreeMap<u8, Vec<Request>> = BTreeMap::new();
    // Sort producers too: determinism does not depend on chunk/archetype migration.
    let mut sorted: Vec<_> = buildings.iter().collect();
    sorted.sort_by_key(|row| row.0.to_bits());
    for (entity, team, kind, transform, health, construction) in sorted {
        if health.current <= 0.0 {
            continue;
        }
        let account = economy.0.entry(team.0).or_default();
        let site = if construction {
            projects.get(entity).ok()
        } else {
            None
        };
        let build_power = site_power(entity, transform.translation, team.0, &flat, any_builders);
        let project = if let Some(site) = site {
            Some((&site.0, build_power, true))
        } else {
            for r in 0..2 {
                account.income[r] += kind.stats().income[r];
            }
            None
        };
        let factory = factories.get(entity).ok();
        let project = project.or_else(|| {
            factory
                .as_ref()
                .and_then(|f| f.queue.front())
                .map(|job| (&job.project, FACTORY_POWER, false))
        });
        if let Some((project, power, construction)) = project.filter(|(p, _, _)| !p.complete()) {
            let work = (power * TICK_SECONDS).min(project.cost.work - project.done);
            let demand = std::array::from_fn(|r| {
                (project.cost.resources[r] * work / project.cost.work)
                    .min((project.cost.resources[r] - project.spent[r]).max(0.0))
            });
            by_team.entry(team.0).or_default().push(Request {
                entity,
                construction,
                work,
                demand,
            });
        }
    }
    for (team, account) in &mut economy.0 {
        for r in 0..2 {
            account.stock[r] =
                (account.stock[r] + account.income[r] * TICK_SECONDS).min(account.capacity[r]);
        }
        let Some(requests) = by_team.get(team) else {
            continue;
        };
        let demands: Vec<_> = requests.iter().map(|r| r.demand).collect();
        let fractions = allocate(account.stock, &demands);
        let mut spent = [0.0; 2];
        for (request, fraction) in requests.iter().zip(fractions) {
            let shortage = std::array::from_fn(|r| {
                request.demand[r] > 0.0
                    && demands.iter().map(|d| d[r]).sum::<f64>() > account.stock[r] + 1e-10
            });
            let advance = |project: &mut Project| {
                project.done = (project.done + request.work * fraction).min(project.cost.work);
                // Resolve sub-nanowork floating point residue; no extra tick
                // or permanent stall at an exactly funded completion.
                if project.cost.work - project.done < 1e-9 {
                    project.done = project.cost.work;
                }
                for r in 0..2 {
                    project.spent[r] = (project.spent[r] + request.demand[r] * fraction)
                        .min(project.cost.resources[r]);
                }
                project.speed = request.work * fraction / TICK_SECONDS;
                project.power = request.work / TICK_SECONDS;
                project.shortage = if fraction < 1.0 - 1e-9 {
                    shortage
                } else {
                    [false; 2]
                };
            };
            if request.construction {
                if let Ok(mut site) = projects.get_mut(request.entity) {
                    advance(&mut site.0);
                }
            } else if let Ok(mut factory) = factories.get_mut(request.entity)
                && let Some(job) = factory.queue.front_mut()
            {
                advance(&mut job.project);
            }
            for (r, paid) in spent.iter_mut().enumerate() {
                *paid += request.demand[r] * fraction;
                account.demand[r] += request.demand[r] / TICK_SECONDS;
            }
        }
        for (r, paid) in spent.iter().enumerate() {
            account.stock[r] = (account.stock[r] - paid).max(0.0);
            account.consumption[r] = paid / TICK_SECONDS;
        }
    }
}
