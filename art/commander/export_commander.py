"""blender --background art/commander/commander.blend --python art/commander/export_commander.py"""
import bpy
from pathlib import Path

repo=Path(bpy.data.filepath).resolve().parents[2]
scene=bpy.data.scenes['Commander_Game']
bpy.context.window.scene=scene
scene.frame_set(0)
for o in bpy.context.selected_objects:o.select_set(False)
for o in bpy.data.collections['Commander_Export'].objects:o.select_set(True)
bpy.context.view_layer.objects.active=bpy.data.objects['Commander']
destination=repo/'assets/models/commander/commander.glb'
bpy.ops.export_scene.gltf(
    filepath=str(destination),export_format='GLB',use_selection=True,
    use_active_scene=True,export_yup=True,export_cameras=False,export_lights=False,
    export_animations=True,export_animation_mode='ACTIONS',
    export_merge_animation='NLA_TRACK',export_force_sampling=False,
    export_anim_slide_to_zero=True,export_extras=True,export_texcoords=False,
    export_apply=False,export_morph=False,export_skins=False,
)
result={'glb':str(destination),'bytes':destination.stat().st_size}
