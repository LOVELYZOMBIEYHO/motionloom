# =========================================
# =========================================
# tools/extract_rig_reference.py
"""Extract only CC0 reference joints; never use this for unknown target models."""
import hashlib
import json
import math
from pathlib import Path
import struct
import sys

IDENTITY = [1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]
def multiply(a,b):
    return [sum(a[k*4+r]*b[c*4+k] for k in range(4)) for c in range(4) for r in range(4)]
def point(m,p):
    return [sum(m[c*4+r]*p[c] for c in range(3))+m[12+r] for r in range(3)]
def quaternion(a,b):
    x,y,z,w=a;X,Y,Z,W=b
    q=[w*X+x*W+y*Z-z*Y,w*Y-x*Z+y*W+z*X,w*Z+x*Y-y*X+z*W,w*W-x*X-y*Y-z*Z]
    n=math.sqrt(sum(v*v for v in q));return [v/n for v in q]
def matrix(node):
    if 'matrix' in node: raise ValueError('Reference must retain decomposed TRS')
    x,y,z,w=node.get('rotation',[0,0,0,1]);s=node.get('scale',[1,1,1]);p=node.get('translation',[0,0,0])
    return [(1-2*y*y-2*z*z)*s[0],(2*x*y+2*w*z)*s[0],(2*x*z-2*w*y)*s[0],0,
            (2*x*y-2*w*z)*s[1],(1-2*x*x-2*z*z)*s[1],(2*y*z+2*w*x)*s[1],0,
            (2*x*z+2*w*y)*s[2],(2*y*z-2*w*x)*s[2],(1-2*x*x-2*y*y)*s[2],0,*p,1]
def canonical(name):
    fixed={'root':'root','pelvis':'hips','spine_01':'spine','spine_02':'chest','spine_03':'upper_chest','neck_01':'neck','Head':'head'}
    if name in fixed:return fixed[name]
    base,side=name.rsplit('_',1)
    body={'clavicle':'shoulder','upperarm':'upper_arm','lowerarm':'forearm','hand':'hand','thigh':'upper_leg','calf':'lower_leg','foot':'foot','ball':'toe','ball_leaf':'toe_end'}
    if base in body:return body[base]+'_'+side
    finger,part=base.split('_',1)
    return finger+'_'+('end' if part=='04_leaf' else str(int(part)))+'_'+side

def extract(path):
    raw=path.read_bytes();size=struct.unpack_from('<I',raw,12)[0];doc=json.loads(raw[20:20+size]);binary=raw[28+size:]
    nodes=doc['nodes'];parents={c:i for i,n in enumerate(nodes) for c in n.get('children',[])};world={};rotations={}
    def visit(i):
        if i in world:return
        parent=parents.get(i)
        if parent is not None:visit(parent)
        world[i]=multiply(world.get(parent,IDENTITY),matrix(nodes[i]))
        rotations[i]=quaternion(rotations.get(parent,[0,0,0,1]),nodes[i].get('rotation',[0,0,0,1]))
    for i in range(len(nodes)):visit(i)
    points=[]
    for i,node in enumerate(nodes):
        if 'mesh' not in node:continue
        for primitive in doc['meshes'][node['mesh']]['primitives']:
            a=doc['accessors'][primitive['attributes']['POSITION']];v=doc['bufferViews'][a['bufferView']]
            offset=v.get('byteOffset',0)+a.get('byteOffset',0);stride=v.get('byteStride',12)
            points.extend(point(world[i],struct.unpack_from('<fff',binary,offset+j*stride)) for j in range(a['count']))
    lo=[min(p[k] for p in points) for k in range(3)];hi=[max(p[k] for p in points) for k in range(3)];height=hi[1]-lo[1];origin=[(lo[0]+hi[0])/2,lo[1],(lo[2]+hi[2])/2]
    indices=doc['skins'][0]['joints'];lookup={i:canonical(nodes[i]['name']) for i in indices}
    joints=[{'id':lookup[i],'referenceName':nodes[i]['name'],'parent':lookup.get(parents.get(i)),
             'endpoint':lookup[i].endswith(('_end_l','_end_r')),
             'position':[round((world[i][12+k]-origin[k])/height,9) for k in range(3)],
             'rotation':[round(x,9) for x in rotations[i]]} for i in indices]
    return {'id':path.stem,'sha256':hashlib.sha256(raw).hexdigest(),'sourceHeight':height,'joints':joints}

if __name__=='__main__':
    if len(sys.argv) != 4:
        raise SystemExit("Expected character1.glb character2.glb output.json")
    expected = [
        ("69591853d817488edaa8fd9bf8fc1d821eaeaf789f8627b3cd23b41c4ed67997", "character1", "Quaternius Universal Animation Library 1 Mannequin", "https://quaternius.com/packs/universalanimationlibrary.html"),
        ("2ee6cc3fe888d9b144afa8cc4b2ab7bfc5d13a0d5b7548df777f61f64ad65fa6", "character2", "Quaternius Universal Animation Library 2 Female Mannequin", "https://quaternius.com/packs/universalanimationlibrary2.html"),
    ]
    references = []
    for path, (digest, rig_id, source, url) in zip(sys.argv[1:3], expected):
        if hashlib.sha256(Path(path).read_bytes()).hexdigest() != digest:
            raise ValueError("Only the two audited CC0 reference files may regenerate this standard")
        reference = extract(Path(path))
        reference.update(id=rig_id, source=source, sourceUrl=url, license="CC0-1.0")
        references.append(reference)
    assert all(len(r['joints'])==65 for r in references)
    assert [(j['id'],j['parent']) for j in references[0]['joints']]==[(j['id'],j['parent']) for j in references[1]['joints']]
    result={'schemaVersion':1,'standardId':'humanoid65_v1','license':'CC0-1.0','source':'Quaternius Universal Animation Library / Universal Animation Library 2','sourceUrl':'https://quaternius.com/packs/universalanimationlibrary.html','coordinateSystem':'rightHandedYUp','positionUnits':'fractionOfMeshHeight','references':references}
    Path(sys.argv[3]).write_text(json.dumps(result,indent=2)+'\n')
