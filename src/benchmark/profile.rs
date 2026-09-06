//! Opt-in per-system-set profiler for benchmark runs (`--profile-systems`).
//!
//! Marker systems bracket the simulation sets and accumulate wall-clock
//! time in [`SystemProfile`]. Spans are measured independently, so an
//! unconstrained system slipping between two markers is attributed to the
//! enclosing span: numbers are approximate, good enough to rank bottlenecks
//! (avoidance vs grid rebuild vs acquisition) but not for accounting.
//! Profiling never touches simulation state, so determinism is unaffected,
//! and the plugin is only added when the flag is set (zero overhead).

use bevy::prelude::*;
use std::time::Instant;

use crate::{
    combat::{
        AcquireTargets, ChaseTargets, DeathSystems, ProjectileSystems, ResolveBehaviour,
        ValidateTargets, WeaponSystems,
    },
    movement::MovementSystems,
    navigation::PlanPaths,
    spatial::{SpatialSystems, apply_avoidance},
};

pub struct ProfilePlugin;

impl Plugin for ProfilePlugin {
    fn build(&self, app: &mut App) {
        // Each marker carries BOTH constraints (after previous, before next):
        // single-sided markers float to the schedule edges under parallel
        // execution and every span would read ~full frame. Sandwiching pins
        // each span around its set. This also pins avoidance ahead of weapon
        // fire, matching the design intent (separate right after movement).
        app.init_resource::<SystemProfile>()
            .add_systems(Update, start_rebuild.before(SpatialSystems))
            .add_systems(
                Update,
                end_rebuild.after(SpatialSystems).before(ValidateTargets),
            )
            .add_systems(
                Update,
                start_validate.after(SpatialSystems).before(ValidateTargets),
            )
            .add_systems(
                Update,
                end_validate.after(ValidateTargets).before(AcquireTargets),
            )
            .add_systems(
                Update,
                start_acquire.after(ValidateTargets).before(AcquireTargets),
            )
            .add_systems(
                Update,
                end_acquire.after(AcquireTargets).before(ResolveBehaviour),
            )
            .add_systems(
                Update,
                start_resolve.after(AcquireTargets).before(ResolveBehaviour),
            )
            .add_systems(
                Update,
                end_resolve.after(ResolveBehaviour).before(PlanPaths),
            )
            .add_systems(Update, start_plan.after(ResolveBehaviour).before(PlanPaths))
            .add_systems(Update, end_plan.after(PlanPaths).before(MovementSystems))
            .add_systems(
                Update,
                start_movement.after(PlanPaths).before(MovementSystems),
            )
            .add_systems(
                Update,
                end_movement.after(MovementSystems).before(ChaseTargets),
            )
            .add_systems(
                Update,
                start_chase.after(MovementSystems).before(ChaseTargets),
            )
            .add_systems(
                Update,
                end_chase.after(ChaseTargets).before(apply_avoidance),
            )
            .add_systems(
                Update,
                start_avoidance.after(ChaseTargets).before(apply_avoidance),
            )
            .add_systems(
                Update,
                end_avoidance.after(apply_avoidance).before(WeaponSystems),
            )
            .add_systems(
                Update,
                start_weapon.after(apply_avoidance).before(WeaponSystems),
            )
            .add_systems(
                Update,
                end_weapon.after(WeaponSystems).before(ProjectileSystems),
            )
            .add_systems(
                Update,
                start_projectile
                    .after(WeaponSystems)
                    .before(ProjectileSystems),
            )
            .add_systems(
                Update,
                end_projectile.after(ProjectileSystems).before(DeathSystems),
            )
            .add_systems(
                Update,
                start_death.after(ProjectileSystems).before(DeathSystems),
            )
            .add_systems(Update, end_death.after(DeathSystems));
    }
}

#[derive(Resource, Default)]
pub struct SystemProfile {
    open: Vec<(&'static str, Instant)>,
    totals: Vec<(&'static str, f64, u64)>,
}

impl SystemProfile {
    fn open(&mut self, label: &'static str) {
        self.open.push((label, Instant::now()));
    }

    fn close(&mut self, label: &'static str) {
        if let Some(index) = self.open.iter().rposition(|(open, _)| *open == label) {
            let (_, start) = self.open.remove(index);
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            match self.totals.iter_mut().find(|(name, _, _)| *name == label) {
                Some(entry) => {
                    entry.1 += ms;
                    entry.2 += 1;
                }
                None => self.totals.push((label, ms, 1)),
            }
        }
    }

    /// Mean milliseconds per measured tick, sorted as registered.
    pub fn means(&self) -> Vec<(&'static str, f64)> {
        self.totals
            .iter()
            .map(|(label, total, calls)| (*label, total / (*calls).max(1) as f64))
            .collect()
    }

    pub fn print(&self, per_team: usize, workload: &str, repeat: usize) {
        let means = self.means();
        let total: f64 = means.iter().map(|(_, mean)| mean).sum();
        println!(
            "profile {per_team} vs {per_team} {workload} repeat={repeat} (mean ms/tick, spans may overlap):"
        );
        for (label, mean) in &means {
            let share = if total > 0.0 {
                mean / total * 100.0
            } else {
                0.0
            };
            println!("  {label:10} {mean:8.3}ms  ({share:5.1}%)");
        }
        println!("  total      {total:8.3}ms measured");
    }
}

macro_rules! span {
    ($start:ident, $end:ident, $label:literal) => {
        fn $start(mut profile: ResMut<SystemProfile>) {
            profile.open($label);
        }
        fn $end(mut profile: ResMut<SystemProfile>) {
            profile.close($label);
        }
    };
}

span!(start_rebuild, end_rebuild, "rebuild");
span!(start_validate, end_validate, "validate");
span!(start_acquire, end_acquire, "acquire");
span!(start_resolve, end_resolve, "resolve");
span!(start_plan, end_plan, "plan");
span!(start_movement, end_movement, "movement");
span!(start_chase, end_chase, "chase");
span!(start_avoidance, end_avoidance, "avoidance");
span!(start_weapon, end_weapon, "weapon");
span!(start_projectile, end_projectile, "projectile");
span!(start_death, end_death, "death");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_accumulate_means_and_ignore_stray_closes() {
        let mut profile = SystemProfile::default();
        profile.close("never-opened");
        profile.open("a");
        profile.close("a");
        profile.open("a");
        profile.close("a");
        profile.open("b");
        profile.close("b");
        assert!(profile.means().iter().all(|(_, mean)| *mean >= 0.0));
        let a = profile
            .means()
            .iter()
            .find(|(label, _)| *label == "a")
            .unwrap()
            .1;
        let b = profile
            .means()
            .iter()
            .find(|(label, _)| *label == "b")
            .unwrap()
            .1;
        assert!(a >= 0.0 && b >= 0.0);
        assert_eq!(profile.totals.len(), 2);
    }
}
