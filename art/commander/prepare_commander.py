"""Run once in commander_v01.blend to build the compact game scene.

Source is preserved; export_commander.py re-exports the resulting .blend.
Blender +Y forward becomes glTF/Bevy -Z forward. Dimensions are baked.
"""
import bpy
import math
from collections import defaultdict
from mathutils import Matrix, Vector

WORKTREE = '/home/mattia/Projects/RRTS-commander'
original = bpy.data.scenes['RRTS | Commander 01']
bpy.context.window.scene = original
bpy.context.view_layer.update()
deps = bpy.context.evaluated_depsgraph_get()
source = list(bpy.data.collections['CMD | Commander v01'].all_objects)
parts = defaultdict(list)
# 2.74 m wide armored hull, 3.60 m long tracks, origin on the ground.
S = Matrix.Diagonal((-.285, -.285, .355, 1.0)) @ Matrix.Translation((0, 0, -.315))
def point(v): return S @ Vector(v)

scene = bpy.data.scenes.new('Commander_Game')
scene.unit_settings.system = 'METRIC'
scene.render.fps = 24
scene.frame_start = 0
scene.frame_end = 48
coll = bpy.data.collections.new('Commander_Export')
scene.collection.children.link(coll)
root = bpy.data.objects.new('Commander', None)
coll.objects.link(root)
root['forward'] = 'Blender +Y / glTF -Z'
root['game_origin'] = 'Ground; mount at (0, -archetype.body.y, 0) below gameplay entity'
root['weapons'] = 'One machine gun + one guided missile launcher; separate yaw pivots'

def node(name, original_pivot=(0,0,.315), parent=root):
    o=bpy.data.objects.new(name,None);coll.objects.link(o);o.parent=parent
    o.location=point(original_pivot) if parent==root else (0,0,0)
    return o

mg=node('MG_Yaw',(0,-2.6,3.93))
mg_recoil=node('MG_Recoil',parent=mg)
rocket=node('Rocket_Yaw',(0,2.6,5.96))
rocket_recoil=node('Rocket_Recoil',parent=rocket)
ramp=node('Ramp',(0,-5.54,.98))
radar=node('Radar',(-1.5,2.8,6.58))
wheels={}
for side in ['L','R']:
    for i,y in enumerate([-5.05,-3.4,-1.7,0,1.7,3.4,5.02]):
        wheels[side,i]=node(f'Wheel_{side}{i}',(-4.40 if side=='L' else 4.40,y,1.40))

materials={m.name:m for m in bpy.data.materials}
def mat(name): return materials['CMD / '+name]
team=mat('Oxide red armor').copy();team.name='TEAM_ARMOR'
materials[team.name]=team
def map_mat(m):
    return team if m and m.name in ['CMD / Oxide red armor','CMD / Vermilion accent'] else m

def add_source(o,target=root,adjust=None):
    e=o.evaluated_get(deps)
    if o.type not in {'MESH','FONT'}: return
    me=e.to_mesh()
    mx=o.matrix_world
    world=[mx@v.co for v in me.vertices]
    if adjust: world=[adjust(v) for v in world]
    # The only non-root parent targets are recoil subnodes.
    pivot=target.location if target.parent==root else target.parent.location if target!=root else Vector((0,0,0))
    verts=[point(v)-pivot for v in world]
    faces=[tuple(p.vertices) for p in me.polygons]
    mats=[map_mat(bpy.data.materials.get(m.name)) if m else None for m in me.materials]
    parts[target].append((verts,faces,mats,[p.material_index for p in me.polygons]))
    e.to_mesh_clear()

closed=Matrix.Rotation(-math.pi/2-math.atan2(.69,2.98),3,'X')
hinge=Vector((0,-5.54,.98))
wheel_counts=defaultdict(int)
for o in source:
    name=o.name
    if o.type not in {'MESH','FONT'}: continue
    if o.name in bpy.data.collections['CMD | Weapons'].objects: continue
    if name.startswith(('Command tower drum','Tower cap','Tower beacon')): continue
    if name.startswith(('L wheel hub','R wheel hub','L axle cap','R axle cap')):
        side=name[0];i=int(name.rsplit(' ',1)[-1]);add_source(o,wheels[side,i]);continue
    if name.startswith(('L road wheel','R road wheel')):
        side=name[0];i=int(name.rsplit(' ',1)[-1]);add_source(o,wheels[side,i]);continue
    if o.parent and o.parent.name=='CMD_RAMP_PIVOT':
        # Shorter door-ramp folds flush across the bay, not over the bridge deck.
        add_source(o,ramp,lambda v:hinge+closed@((v-hinge)*Vector((1,.77,.77))))
        continue
    if name.startswith(('Comms','Mast collar','Amber antenna')):
        add_source(o,adjust=lambda v:v+Vector((-1.40,.32,0)))
        continue
    if name.startswith('Crane'):
        # Stow the service boom above the deck, within the existing unit width.
        add_source(o,adjust=lambda v:Vector((2.95+(v.x-2.95)*.54,v.y,v.z)))
        continue
    add_source(o)

# New weapon geometry is authored with temporary objects, then baked into groups.
bpy.context.window.scene=scene
temp=bpy.data.collections.new('Construction helpers');scene.collection.children.link(temp)
def bake_new(o,target,material,bevel=0):
    for c in list(o.users_collection):c.objects.unlink(o)
    temp.objects.link(o)
    o.data.materials.append(material)
    if bevel:
        b=o.modifiers.new('Bevel','BEVEL');b.width=bevel;b.segments=1
    bpy.context.view_layer.update()
    global deps
    deps=bpy.context.evaluated_depsgraph_get()
    add_source(o,target)
    bpy.data.objects.remove(o,do_unlink=True)

def box(loc,size,material,target,bevel=.04):
    bpy.ops.mesh.primitive_cube_add(size=1,location=loc)
    o=bpy.context.object
    for v in o.data.vertices:v.co*=Vector(size)
    bake_new(o,target,material,bevel)

def cyl(loc,r,d,material,target,axis='Z'):
    bpy.ops.mesh.primitive_cylinder_add(vertices=12,radius=r,depth=d,location=loc)
    o=bpy.context.object
    if axis=='Y':o.rotation_euler.x=math.pi/2
    bake_new(o,target,material,.018 if r>.12 else 0)

gray=mat('Graphite hull');steel=mat('Steel edging');black=mat('Recesses and rubber');yellow=mat('Safety ochre');light=mat('Amber running lights')
cyl((0,-2.6,3.91),.79,.22,steel,root)
cyl((0,-2.6,4.09),.70,.21,black,mg)
box((0,-2.6,4.43),(1.63,1.55,.65),team,mg,.14)
box((0,-2.56,4.81),(1.19,1.04,.12),gray,mg,.05)
box((0,-3.39,4.45),(.88,.22,.37),gray,mg,.06)
# Single machine gun: one perforated cooling jacket, ammo box and one muzzle.
box((.93,-2.55,4.37),(.45,.87,.51),steel,mg,.06)
for y in [-2.83,-2.63,-2.43]: box((1.18,y,4.4),(.06,.085,.30),yellow,mg,.01)
cyl((0,-3.95,4.48),.19,1.10,steel,mg_recoil,'Y')
for y in [-3.66,-3.93,-4.2]:
    for x in [-.181,.181]:box((x,y,4.48),(.022,.12,.095),black,mg_recoil,.005)
cyl((0,-4.64,4.48),.105,.45,gray,mg_recoil,'Y')
cyl((0,-4.92,4.48),.145,.20,steel,mg_recoil,'Y')
cyl((0,-5.025,4.48),.094,.017,black,mg_recoil,'Y')
box((.56,-3.29,4.63),(.16,.1,.1),light,mg,.015)

# Rear guided launcher is a single box pod, with four tubes sharing one pivot.
cyl((0,2.6,5.94),.70,.20,steel,root)
cyl((0,2.6,6.11),.59,.21,black,rocket)
box((0,2.6,6.46),(1.89,1.57,.65),gray,rocket_recoil,.12)
box((0,2.70,6.86),(1.75,1.4,.16),team,rocket_recoil,.06)
for x in [-.61,-.20,.20,.61]:
    cyl((x,1.795,6.49),.167,.10,yellow,rocket_recoil,'Y')
    cyl((x,1.736,6.49),.116,.027,black,rocket_recoil,'Y')
    # Visible missile nose is recessed in the same four-tube launcher.
    cyl((x,1.718,6.49),.061,.012,steel,rocket_recoil,'Y')
box((0,3.43,6.46),(1.4,.12,.27),black,rocket_recoil,.02)
box((0,2.6,6.99),(.27,.32,.1),yellow,rocket_recoil,.02)

# Small sensor scanner provides a restrained idle loop without moving the hull.
cyl((-1.5,2.8,6.42),.18,.29,steel,root)
box((-1.5,2.8,6.69),(.88,.18,.25),gray,radar,.04)
box((-1.5,2.685,6.69),(.62,.035,.11),light,radar,.015)

for target,chunks in parts.items():
    verts=[];faces=[];slots=[];indices=[]
    for vs,fs,ms,mi in chunks:
        start=len(verts);verts.extend(vs)
        remap=[]
        for m in ms:
            m=m or gray
            if m not in slots:slots.append(m)
            remap.append(slots.index(m))
        faces.extend(tuple(start+j for j in f) for f in fs)
        indices.extend(remap[i] if remap else 0 for i in mi)
    me=bpy.data.meshes.new(target.name+'_mesh');me.from_pydata(verts,[],faces)
    for m in slots:me.materials.append(m)
    for p,i in zip(me.polygons,indices):p.material_index=i
    ob=bpy.data.objects.new(target.name+'_Geometry',me);coll.objects.link(ob);ob.parent=target

bpy.data.collections.remove(temp)

# Socket nodes are kept as semantic attachment points. Code constants mirror them.
def socket(name,position,parent):
    ob=node(name,parent=parent)
    ob.location=point(position)-parent.location
    return ob
socket('Muzzle_Primary',(0,-5.04,4.48),mg)
socket('Muzzle_Secondary',(0,1.69,6.49),rocket)

def animate(obj,clip,prop,keys):
    base=getattr(obj,prop).copy()
    for frame,value in keys:
        setattr(obj,prop,value);obj.keyframe_insert(data_path=prop,frame=frame)
    ad=obj.animation_data;action=ad.action;action.name=clip+'_'+obj.name
    for layer in action.layers:
        for strip in layer.strips:
            for bag in strip.channelbags:
                for curve in bag.fcurves:
                    for key in curve.keyframe_points:key.interpolation='LINEAR'
    track=ad.nla_tracks.new();track.name=clip
    strip=track.strips.new(action.name,int(action.frame_range[0]),action)
    strip.extrapolation='NOTHING'
    ad.action=None
    setattr(obj,prop,base)

animate(radar,'Idle','rotation_euler',[(0,(0,0,-.45)),(24,(0,0,.45)),(48,(0,0,-.45))])
for (side,i),wheel in wheels.items():
    # One loop = circumference at wheel radius .24m, speed set from actual travel.
    animate(wheel,'Move_'+side,'rotation_euler',[(f,(math.tau*f/24,0,0)) for f in range(0,25,3)])
animate(mg_recoil,'Fire_Primary','location',[(0,(0,0,0)),(1,(0,-.048,0)),(3,(0,-.028,0)),(6,(0,0,0))])
animate(rocket_recoil,'Fire_Secondary','location',[(0,(0,0,0)),(2,(0,-.036,0)),(6,(0,-.020,0)),(12,(0,0,0))])
animate(ramp,'Deploy','rotation_euler',[(0,(0,0,0)),(24,(-1.80,0,0))])
scene.frame_set(0)
scene['animation_clips']='Idle, Move_L, Move_R, Fire_Primary, Fire_Secondary, Deploy'
scene['primary_pivot_bevy']=list((mg.location.x,mg.location.z,-mg.location.y))
scene['secondary_pivot_bevy']=list((rocket.location.x,rocket.location.z,-rocket.location.y))

# Studio copies are never exported.
studio=bpy.data.collections.new('Game Preview Studio');scene.collection.children.link(studio)
for old in bpy.data.collections['Studio | presentation'].objects:
    o=old.copy()
    if o.data:o.data=o.data.copy()
    studio.objects.link(o)
    if o.type=='CAMERA':
        o.location=(-4.8,6.9,5.0);o.rotation_euler=(Vector((0,0,1.35))-o.location).to_track_quat('-Z','Y').to_euler();o.data.ortho_scale=6.4;scene.camera=o
    elif o.type=='LIGHT':
        o.location=point(old.location);o.rotation_euler=(Vector((0,0,.9))-o.location).to_track_quat('-Z','Y').to_euler();o.data.energy*=.09;o.data.size*=.30
    elif o.type=='MESH':o.location.z=-.05
scene.world=original.world
scene.render.engine='BLENDER_EEVEE';scene.render.resolution_x=1300;scene.render.resolution_y=1100;scene.render.resolution_percentage=100
scene.view_settings.view_transform='AgX';scene.view_settings.look='AgX - Medium High Contrast'
scene.render.filepath=WORKTREE+'/art/commander/preview.png'
for o in bpy.context.selected_objects:o.select_set(False)
root.select_set(True);bpy.context.view_layer.objects.active=root
for ar in bpy.context.screen.areas:
    if ar.type=='VIEW_3D':ar.spaces.active.region_3d.view_perspective='CAMERA'

bpy.ops.wm.save_as_mainfile(filepath=WORKTREE+'/art/commander/commander.blend')
result={'scene':scene.name,'export_objects':len(coll.objects),'primary_pivot_bevy':list(scene['primary_pivot_bevy']),'secondary_pivot_bevy':list(scene['secondary_pivot_bevy'])}
