"""Stage an unchanged H3 model with only its supported child-result memo switch disabled."""
import argparse,ast,hashlib,json,shutil
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--source',type=Path,required=True);p.add_argument('--provenance',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
a=p.parse_args();assert not a.out.exists();provenance=json.loads(a.provenance.read_text())
sha=lambda data:hashlib.sha256(data).hexdigest()
for name,expected in provenance['files'].items():assert sha((a.source/name).read_bytes())==expected,name
shutil.copytree(a.source,a.out)
path=a.out/'minimax-h3/h3.py';before=path.read_text();tree=ast.parse(before)
fn,=[node for node in tree.body if isinstance(node,ast.AsyncFunctionDef) and node.name=='fl2va_piece']
call,=[node for node in fn.decorator_list if isinstance(node,ast.Call) and isinstance(node.func,ast.Name) and node.func.id=='invocable']
keyword,=[k for k in call.keywords if k.arg=='memoize'];assert isinstance(keyword.value,ast.Constant) and keyword.value.value is True
node=keyword.value;lines=before.splitlines(keepends=True);assert node.lineno==node.end_lineno
line=lines[node.lineno-1];assert line[node.col_offset:node.end_col_offset]=='True'
lines[node.lineno-1]=line[:node.col_offset]+'False'+line[node.end_col_offset:];after=''.join(lines);path.write_text(after)
changed=ast.parse(after);changed_fn,=[n for n in changed.body if isinstance(n,ast.AsyncFunctionDef) and n.name=='fl2va_piece']
changed_call,=[n for n in changed_fn.decorator_list if isinstance(n,ast.Call) and isinstance(n.func,ast.Name) and n.func.id=='invocable']
changed_kw,=[k for k in changed_call.keywords if k.arg=='memoize'];assert changed_kw.value.value is False;changed_kw.value.value=True
assert ast.dump(tree,include_attributes=False)==ast.dump(changed,include_attributes=False)
files={name:sha((a.out/name).read_bytes()) for name in provenance['files']};assert [name for name in files if files[name]!=provenance['files'][name]]==['minimax-h3/h3.py']
r={'original_source':provenance['source'],'original_provenance_sha256':sha(a.provenance.read_bytes()),'files':files,'change':'fl2va_piece invocable memoize=True to False only','ast_equal_after_restoring_single_boolean':True,'scope':'ordinary public shot/child execution; identical function bodies, model requests, kernels and numerical settings; memo-policy difference explicit'}
(a.out.parent/(a.out.name+'-provenance.json')).write_text(json.dumps(r,indent=2)+'\n');print(json.dumps({'source':str(a.out),'files':len(files),'change':r['change']}))
