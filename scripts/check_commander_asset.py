"""Sub-second GLB contract check; no Blender, renderer, or game simulation needed."""
import json
import math
import struct
from pathlib import Path

path = Path(__file__).resolve().parents[1] / 'assets/models/commander/commander.glb'
data = path.read_bytes()
magic, version, size = struct.unpack_from('<4sII', data)
assert magic == b'glTF' and version == 2 and size == len(data)
length, kind = struct.unpack_from('<I4s', data, 12)
assert kind == b'JSON'
doc = json.loads(data[20:20 + length])
nodes = {n['name']: n for n in doc['nodes']}
animations = {a['name']: a for a in doc['animations']}
required = {'Idle', 'Move_L', 'Move_R', 'Fire_Primary', 'Fire_Secondary', 'Deploy'}
assert set(animations) == required, set(animations)
assert {'Commander', 'MG_Yaw', 'Rocket_Yaw', 'MG_Recoil', 'Rocket_Recoil',
        'Ramp', 'Muzzle_Primary', 'Muzzle_Secondary'} <= nodes.keys()
assert not doc.get('cameras') and not doc.get('images') and not doc.get('skins')
assert all('uri' not in b for b in doc['buffers']), 'GLB must be self-contained'
assert 'TEAM_ARMOR' in {m.get('name') for m in doc['materials']}
assert len(doc['nodes']) < 60, 'Static parts must be merged before export'
for name, animation in animations.items():
    assert animation['channels'], name
    for channel in animation['channels']:
        target = doc['nodes'][channel['target']['node']]['name']
        assert target not in {'Commander', 'MG_Yaw', 'Rocket_Yaw'}, (name, target)
        accessor = doc['accessors'][animation['samplers'][channel['sampler']]['input']]
        assert accessor['min'][0] == 0 and accessor['max'][0] > 0
for node in doc['nodes']:
    assert all(math.isfinite(x) for key in ['translation', 'rotation', 'scale']
               for x in node.get(key, []))
for muzzle in ['Muzzle_Primary', 'Muzzle_Secondary']:
    assert nodes[muzzle]['translation'][2] < 0, 'Muzzles must point along Bevy -Z'
triangles = sum(doc['accessors'][p['indices']]['count'] // 3
                for mesh in doc['meshes'] for p in mesh['primitives'])
print(json.dumps({'file': str(path), 'bytes': size, 'nodes': len(nodes),
                  'triangles': triangles,
                  'clips': {n: len(a['channels']) for n, a in animations.items()},
                  'sockets': {n: nodes[n].get('translation') for n in
                              ['MG_Yaw', 'Rocket_Yaw', 'Muzzle_Primary', 'Muzzle_Secondary']}}, indent=2))
