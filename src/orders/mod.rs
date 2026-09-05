use bevy::{prelude::*, window::PrimaryWindow};

use crate::{
    camera::RtsCamera,
    movement::queue_move,
    navigation::NavGrid,
    picking::ground_position,
    selection::{Selected, SelectionSystems},
    units::Unit,
};

pub struct OrderPlugin;

impl Plugin for OrderPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FormationSettings { spacing: 2.5 })
            .add_systems(PostUpdate, issue_move_order.after(SelectionSystems));
    }
}

#[derive(Resource)]
pub struct FormationSettings {
    pub spacing: f32,
}

fn issue_move_order(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<RtsCamera>>,
    selected: Query<(Entity, &Unit), With<Selected>>,
    settings: Res<FormationSettings>,
    grid: Res<NavGrid>,
) {
    if !window.focused || !mouse.just_pressed(MouseButton::Right) || selected.is_empty() {
        return;
    }
    let (camera, transform) = *camera;
    let Some(center) = window
        .cursor_position()
        .and_then(|cursor| ground_position(camera, transform, cursor))
    else {
        return;
    };
    let mut units: Vec<_> = selected.iter().collect();
    units.sort_unstable_by_key(|(_, unit)| unit.0);
    let Some(slots) = grid.formation(units.len(), center, settings.spacing) else {
        return;
    };
    for (slot, (entity, _)) in slots.into_iter().zip(units) {
        queue_move(&mut commands.entity(entity), slot);
    }
}
