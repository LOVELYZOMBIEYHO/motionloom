# =========================================
# =========================================
# tools/migrate_geometry_assets.py
"""One-shot offline migration. The runtime never loads the removed DSL forms."""
from pathlib import Path
import re, json, argparse, hashlib

LEGACY={'PrimitiveAsset','SweepAsset','MeshAsset','HeadAsset','HairAsset'}
APPEARANCE={'id','material','color','materialSeed','collision','collider','colliderSize','colliderRadius','colliderHeight','colliderScale','colliderOffset','colliderRotation','colliderMargin','collisionGroup','collisionMask','friction','restitution','density'}

def tags(source):
    result=[];i=0
    while i<len(source):
        if source.startswith('<!--',i):
            end=source.find('-->',i+4);i=len(source) if end<0 else end+3;continue
        if source[i]!='<' or not re.match(r'</?[A-Za-z]',source[i:i+4]):i+=1;continue
        start=i;i+=1;quote='';depth=0
        while i<len(source):
            c=source[i]
            if quote:
                if c==quote and source[i-1]!='\\':quote=''
            elif c in '\"\'':quote=c
            elif c=='{':depth+=1
            elif c=='}':depth-=1
            elif c=='>' and depth==0:break
            i+=1
        raw=source[start:i+1];m=re.match(r'<(/?)(\w+)',raw)
        if m:result.append({'name':m[2],'close':bool(m[1]),'start':start,'end':i+1,'raw':raw,'self':raw.rstrip().endswith('/>')})
        i+=1
    return result

def attrs(raw):
    result={};pos=re.match(r'<\w+',raw).end()
    while pos<len(raw):
        m=re.match(r'\s*([\w]+)\s*=\s*',raw[pos:])
        if not m:break
        key=m[1];pos+=m.end();start=pos
        if raw[pos] in '\"\'':
            quote=raw[pos];pos+=1
            while pos<len(raw) and (raw[pos]!=quote or raw[pos-1]=='\\'):pos+=1
            pos+=1
        elif raw[pos]=='{':
            depth=1;pos+=1;quote=''
            while pos<len(raw) and depth:
                c=raw[pos]
                if quote:
                    if c==quote and raw[pos-1]!='\\':quote=''
                elif c in '\"\'':quote=c
                elif c=='{':depth+=1
                elif c=='}':depth-=1
                pos+=1
        else:
            while pos<len(raw) and not raw[pos].isspace() and raw[pos] not in '/>':pos+=1
        result[key]=raw[start:pos]
    return result

def literal(value):return value.strip('"\'')
def element(name,values,selfclose=True):return '<'+name+''.join(f' {k}={v}' for k,v in values.items())+(' />' if selfclose else '>')
def extent(all_tags,index):
    tag=all_tags[index]
    if tag['self']:return tag['end']
    depth=1
    for other in all_tags[index+1:]:
        if other['name']==tag['name']:
            depth+= -1 if other['close'] else (0 if other['self'] else 1)
            if depth==0:return other['end']
    raise ValueError(f"Unclosed {tag['name']}")

def child_blocks(body,names):
    result=[];patches=[];all_tags=tags(body)
    for i,t in enumerate(all_tags):
        if t['name'] not in names or t['close']:continue
        end=extent(all_tags,i);result.append((t['name'],body[t['start']:end]));patches.append((t['start'],end,''))
    for a,b,v in reversed(patches):body=body[:a]+v+body[b:]
    return body,result

def migrate_head_controls(source):
    all_tags=tags(source);patches=[]
    for i,t in enumerate(all_tags):
        if t['name']!='GeometryAsset' or t['close']:continue
        end=extent(all_tags,i);inner=source[t['end']:end-len('</GeometryAsset>')]
        controls=tags(inner);changes=[];modifiers=[];uv=''
        for child in controls:
            if child['close'] or child['name'] not in ('HeadCage','FacialCage'):continue
            a=attrs(child['raw']);level=a.pop('subdivision',None)
            if level is None:continue
            level=literal(level)
            if level!='0':modifiers.append('<Subdivision levels="'+level+'" scheme="catmullClark" />')
            mode=literal(a.pop('uvMode','"frontBack"')).lower()
            if mode=='fallbackxy':uv='<UV mode="planar" uAxis="x" vAxis="y" />\n'
            changes.append((child['start'],child['end'],element(child['name'],a,child['self'])))
        if not changes:continue
        for start,stop,text in reversed(changes):inner=inner[:start]+text+inner[stop:]
        if modifiers:
            # UV is generated before subdivision to preserve authored mapping.
            op='\n'.join(modifiers)
            if '<Modifiers>' in inner:inner=inner.replace('<Modifiers>','<Modifiers>\n'+op,1)
            else:inner+='\n<Modifiers>\n'+op+'\n</Modifiers>\n'
        if uv:inner='\n'+uv+inner
        patches.append((t['end'],end-len('</GeometryAsset>'),inner))
    for a,b,v in reversed(patches):source=source[:a]+v+source[b:]
    return source

def migrate(source):
    source=re.sub(r'<Primitive\b([^>]*?)>\s*</Primitive>', lambda m:'<Primitive'+m[1]+' />', source)
    source=re.sub(r'<Primitive\b([^>]*?)>\s*</Primitive>', lambda m:'<Primitive'+m[1]+' />', source)
    all_tags=tags(source); mappings={};patches=[];default=False;canonical={}; existing_ids={literal(attrs(t['raw']).get('id','')) for t in all_tags if not t['close']}
    for t in all_tags:
        if t['name']=='MaterialAsset' and not t['close']:
            a=attrs(t['raw']);mapping=literal(a.pop('mapping','"uv"')).lower();mappings[literal(a.get('id',''))]=mapping
            if 'mapping' in attrs(t['raw']):patches.append((t['start'],t['end'],element('MaterialAsset',a)))
    for i,t in enumerate(all_tags):
        if t['close'] or t['name'] not in LEGACY:continue
        a=attrs(t['raw'])
        if t['name']=='MeshAsset' and 'geometry' in a:continue
        if 'id' not in a:continue
        identity=literal(a['id']);end=extent(all_tags,i);inner='' if t['self'] else source[t['end']:end-len('</'+t['name']+'>')]
        material=a.get('material','"geometry_default"');default|='material' not in a
        binding={k:v for k,v in a.items() if k in APPEARANCE};binding['material']=material
        gattrs={k:v for k,v in a.items() if k not in APPEARANCE};name=t['name'].removesuffix('Asset')
        uv={};mapping=mappings.get(literal(material),'uv')
        if mapping!='uv':uv['mode']='"box"'
        if name=='Sweep':
            uv['mode']='"'+literal(gattrs.pop('uvMode','"distance"'))+'"' if mapping=='uv' else '"box"'
            if 'uvScale' in gattrs:uv['scale']=gattrs.pop('uvScale')
        if name=='Head':
            inner=re.sub(r'<(HeadCage|FacialCage)\b([^>]*?)(/?>)', lambda m:m[0] if 'subdivision=' in m[2] else '<'+m[1]+m[2]+' subdivision=\"'+('2' if m[1]=='HeadCage' else '1')+'\"'+m[3],inner)
        if name in ('Head','Hair'):
            if 'seed' in gattrs:binding['materialSeed']=gattrs.pop('seed')
        inner,extras=child_blocks(inner,{'Modifiers','MeshBuild','LOD'})
        if name=='Primitive' and literal(gattrs.get('shape','')).lower() in ('loft','ribbon'):
            body=inner.strip()
        elif name=='Mesh':
            levels=literal(gattrs.pop('subdivision','"0"'));gattrs.pop('subdivisionScheme',None)
            body='<Mesh>'+inner+'</Mesh>'
            if levels!='0':extras.insert(0,('Modifiers',f'<Modifiers>\n<Subdivision levels="{levels}" scheme="catmullClark" />\n</Modifiers>'))
        elif name=='Primitive' and not inner.strip():body=element(name,gattrs)
        elif name=='Primitive' and not inner.strip():body=element(name,gattrs)
        else:body=element(name,gattrs,t['self'])+('' if t['self'] else inner+'</'+name+'>')
        for _,extra in extras:
            extra=re.sub(r'<Subdivision\b([^>]*?)(/>)',lambda m:'<Subdivision'+m[1]+(' scheme="linear" ' if 'scheme=' not in m[1] else '')+m[2],extra)
            body+='\n'+extra
        if uv:body+='\n'+element('UV',uv)
        # Equivalent geometry shares one definition even across material variants.
        key=json.dumps([(t['name'],t['close'],sorted((k,re.sub(r'\s+','',v) if v.startswith('{') else v) for k,v in attrs(t['raw']).items())) for t in tags(body) if not t['close']] + [(t['name'],True) for t in tags(body) if t['close']])
        if key in canonical:gid=canonical[key];geometry=''
        else:
            gid=identity+'_geometry'
            while gid in existing_ids:gid+='_shape'
            existing_ids.add(gid);canonical[key]=gid
            geometry='<GeometryAsset id="'+gid+'">\n'+body+'\n</GeometryAsset>\n'
        binding['geometry']='"'+gid+'"'
        indent=source[source.rfind('\n',0,t['start'])+1:t['start']]
        indent=indent if not indent.strip() else ''
        replacement=geometry+element('MeshAsset',binding)
        replacement=replacement.replace('\n','\n'+indent)
        patches.append((t['start'],end,replacement))
    if default and 'geometry_default' not in existing_ids:
        assets=next((t for t in all_tags if t['name']=='Assets' and not t['close']),None)
        if assets:patches.append((assets['end'],assets['end'],'\n<MaterialAsset id="geometry_default" shading="pbr" roughness="0.82" specular="1" emissiveStrength="1" />'))
    for a,b,v in sorted(patches,reverse=True):source=source[:a]+v+source[b:]
    return migrate_head_controls(source)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('paths',nargs='+');parser.add_argument('--check',action='store_true');args=parser.parse_args()
    changed=[]
    for value in args.paths:
        path=Path(value);files=list(path.rglob('*.motionloom')) if path.is_dir() else [path]
        for p in files:
            old=p.read_text();new=migrate(old)
            if old!=new:
                changed.append(str(p))
                if not args.check:p.write_text(new)
    print(json.dumps({'changed':len(changed),'files':changed},indent=2))
if __name__=='__main__':main()
