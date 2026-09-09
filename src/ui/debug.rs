//! Opt-in presentation diagnostics. Never used by simulation decisions.
use crate::{
    selection::Selected,
    session::ViewState,
    units::{Team, Unit},
};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugTool {
    Performance,
    Simulation,
    Navigation,
    Economy,
    Decisions,
    Knowledge,
    Units,
    Buildings,
    Bases,
    Routes,
    Inspector,
}
impl DebugTool {
    pub const ALL: [Self; 11] = [
        Self::Performance,
        Self::Simulation,
        Self::Navigation,
        Self::Economy,
        Self::Decisions,
        Self::Knowledge,
        Self::Units,
        Self::Buildings,
        Self::Bases,
        Self::Routes,
        Self::Inspector,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Performance => "Performance",
            Self::Simulation => "Simulation",
            Self::Navigation => "Navigation",
            Self::Economy => "Economy",
            Self::Decisions => "AI decisions",
            Self::Knowledge => "AI knowledge",
            Self::Units => "AI unit markers",
            Self::Buildings => "AI building markers",
            Self::Bases => "AI base markers",
            Self::Routes => "Selected routes",
            Self::Inspector => "Entity inspector",
        }
    }
}
#[derive(Resource, Clone, Default)]
pub struct DebugSettings {
    pub open: bool,
    pub tools: [bool; 11],
    pub team: Option<u8>,
    pub selected_only: bool,
    pub full_view: bool,
}
impl DebugSettings {
    pub fn on(&self, tool: DebugTool) -> bool {
        self.tools[tool as usize]
    }
    pub fn toggle(&mut self, tool: DebugTool) {
        self.tools[tool as usize] ^= true;
    }
    pub fn count(&self) -> usize {
        self.tools.iter().filter(|v| **v).count() + usize::from(self.full_view)
    }
    pub fn allows(&self, view: &ViewState, team: u8) -> bool {
        self.team.is_none_or(|t| t == team) && view.permits_private_data(team)
    }
}
#[derive(Resource, Default)]
pub struct Inspected(pub Option<Entity>);

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw_overlays(
    mut gizmos: Gizmos,
    settings: Res<DebugSettings>,
    view: Res<ViewState>,
    snapshots: Option<Res<crate::ai::AiSnapshots>>,
    config: Option<Res<crate::ai::AiConfig>>,
    scenario: Res<crate::scenario::Scenario>,
    inspected: Res<Inspected>,
    selected: Query<(), With<Selected>>,
    routes: Query<(Entity, &Team, &Transform, &crate::navigation::Route), With<Unit>>,
) {
    let marked = |e| !settings.selected_only || inspected.0 == Some(e) || selected.contains(e);
    let color = |team| {
        if team == 0 {
            Color::srgb(0.25, 0.7, 1.0)
        } else {
            Color::srgb(1.0, 0.4, 0.3)
        }
    };
    let ring = |pos: Vec3| {
        Isometry3d::new(
            pos + Vec3::Y * 0.2,
            Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        )
    };
    if let Some(snapshots) = snapshots {
        for (team, s) in &snapshots.0 {
            if !settings.allows(&view, *team) {
                continue;
            }
            if settings.on(DebugTool::Units) {
                for u in s.my_units.iter().filter(|u| marked(u.entity)) {
                    gizmos.circle(ring(u.pos), 1.5, color(*team));
                }
            }
            if settings.on(DebugTool::Buildings) {
                for b in s.my_buildings.iter().filter(|b| marked(b.entity)) {
                    gizmos.cube(
                        Transform::from_translation(b.pos + Vec3::Y)
                            .with_scale(Vec3::new(3.0, 2.0, 3.0)),
                        color(*team),
                    );
                }
            }
            if settings.on(DebugTool::Knowledge) {
                for enemy in s.visible_enemies.iter().filter(|e| marked(e.entity)) {
                    gizmos.circle(ring(enemy.pos), 2.0, Color::srgb(1.0, 0.85, 0.2));
                }
                if !settings.selected_only {
                    for memory in &s.memory {
                        gizmos.circle(ring(memory.pos), 2.5, Color::srgba(1.0, 0.6, 0.2, 0.5));
                    }
                }
            }
        }
    }
    if settings.on(DebugTool::Bases)
        && !settings.selected_only
        && let Some(config) = config
    {
        for brain in &config.teams {
            if settings.allows(&view, brain.team) {
                gizmos.circle(
                    ring(scenario.center(brain.team as usize)),
                    4.0,
                    color(brain.team),
                );
            }
        }
    }
    if settings.on(DebugTool::Routes) {
        for (entity, team, transform, route) in &routes {
            if !settings.allows(&view, team.0)
                || !(selected.contains(entity) || inspected.0 == Some(entity))
            {
                continue;
            }
            let mut from = transform.translation;
            for point in route.points.iter().skip(route.next) {
                gizmos.line(from.with_y(0.3), point.with_y(0.3), color(team.0));
                from = *point;
            }
        }
    }
}
/// Inspection has its own neutral ring; it never inserts Selected.
pub fn draw_inspection(
    mut gizmos: Gizmos,
    inspected: Res<Inspected>,
    entities: Query<(
        &Transform,
        &Team,
        Option<&Visibility>,
        Option<&crate::combat::Health>,
    )>,
) {
    if let Some(entity) = inspected.0
        && let Ok((transform, _, visibility, health)) = entities.get(entity)
        && visibility.is_none_or(|v| *v != Visibility::Hidden)
        && health.is_none_or(|h| h.current > 0.0)
    {
        gizmos.circle(
            Isometry3d::new(
                transform.translation + Vec3::Y * 0.2,
                Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            ),
            2.0,
            Color::srgb(0.95, 0.9, 0.6),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_does_not_enable_tools_and_tools_are_independent() {
        let mut settings = DebugSettings {
            open: true,
            ..default()
        };
        assert_eq!(settings.count(), 0);
        settings.toggle(DebugTool::Navigation);
        settings.toggle(DebugTool::Inspector);
        settings.open = false;
        assert_eq!(settings.count(), 2);
        assert!(!settings.on(DebugTool::Units));
        settings.toggle(DebugTool::Navigation);
        assert!(settings.on(DebugTool::Inspector));
    }
    #[test]
    fn filters_cannot_bypass_fog() {
        let settings = DebugSettings {
            team: Some(1),
            ..default()
        };
        assert!(!settings.allows(&ViewState::default(), 1));
        assert!(settings.allows(
            &ViewState {
                team: 0,
                fog_on: false
            },
            1
        ));
    }
}
