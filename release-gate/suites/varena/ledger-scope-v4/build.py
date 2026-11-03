"""Freeze exact ledger modules and build tiny CPU-only peers in a task-private target."""
import argparse, hashlib, json, subprocess
from pathlib import Path

parser=argparse.ArgumentParser()
parser.add_argument('--repository',type=Path,required=True)
parser.add_argument('--control',required=True)
parser.add_argument('--candidate',required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
args.output.mkdir(parents=True,exist_ok=False)
helper=Path(__file__).with_name('peer.rs').read_bytes()
sha=lambda b:hashlib.sha256(b).hexdigest()
receipts={}
for label,ref in [('v3',args.control),('v4',args.candidate)]:
    commit=subprocess.check_output(['git','-C',str(args.repository),'rev-parse',ref],text=True).strip()
    root=args.output/label;(root/'src/ledger').mkdir(parents=True)
    (root/'src/main.rs').write_bytes(helper)
    files={}
    for relative,target in [('crates/varena/src/ledger.rs','src/ledger.rs'),('crates/varena/src/ledger/os.rs','src/ledger/os.rs')]:
        data=subprocess.check_output(['git','-C',str(args.repository),'show',f'{commit}:{relative}'])
        (root/target).write_bytes(data);files[relative]=sha(data)
    (root/'Cargo.toml').write_text(f'''[package]
name = "ledger-scope-{label}"
version = "0.1.0"
edition = "2024"
[workspace]
[dependencies]
libc = "=0.2.190"
thiserror = "=2.0.21"
serde_json = "=1.0.154"
rustix = {{ version = "=1.1.5", default-features = false, features = ["std", "thread"] }}
[profile.dev]
debug = 0
''')
    receipts[label]={'commit':commit,'source_files':files,'client_sha256':sha(helper),'project':str(root.resolve())}
(args.output/'source.json').write_text(json.dumps({'peers':receipts,'gpu_executed':False,'module_only':True},indent=2)+'\n')
print(json.dumps({'prepared':str(args.output),'build_performed':False}))
