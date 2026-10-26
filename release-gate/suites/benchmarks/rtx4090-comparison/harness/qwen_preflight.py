"""Fail-closed comparison of actual Qwen2.1 canonical stored-data receipts.
python qwen_preflight.py qwen-cozy-identity.json qwen-comfy-identity.json matched-qwen.json
"""
import hashlib
import json
import math
import sys
from pathlib import Path

EXPECTED='sha256:78705de45f7421fde51a0196913b11f1f7e3c95d468881d762d18af50da6fec3'
COMFY='fb3befd168731438f88bc96e0920a538e8649878'


def compare(cozy,comfy):
    assert cozy['tensorfs_checkpoint']==EXPECTED
    assert comfy['comfy_commit']==COMFY and comfy['model_class']=='QwenImage21'
    assert cozy['torch']==comfy['torch']=='2.14.1+cu130'
    assert cozy['torch_git'] and cozy['torch_git']==comfy['torch_git']
    assert (cozy['steps'],cozy['width'],cozy['height'])==(30,1024,1024)
    assert (comfy['steps'],comfy['width'],comfy['height'])==(30,1024,1024)
    assert set(cozy['components'])==set(comfy['components'])=={'transformer','text_encoder','vae'}
    errors=[];counts={};ignored=[]
    for component in ('transformer','text_encoder','vae'):
        a=dict(cozy['components'][component]);b=dict(comfy['components'][component])
        # Both real consumers use hidden states and do not execute the LM output head.
        if component=='text_encoder':
            for key in ('lm_head.weight',):
                if key in a or key in b:
                    ignored.append({'component':component,'key':key,'reason':'not executed by either image conditioner'})
                a.pop(key,None);b.pop(key,None)
        for key in sorted(set(a)-set(b)):errors.append({'component':component,'key':key,'reason':'missing_from_comfy'})
        for key in sorted(set(b)-set(a)):errors.append({'component':component,'key':key,'reason':'extra_in_comfy'})
        counts[component]={'cozy':len(a),'comfy':len(b),'cozy_encoded':sum(v['encoding'] is not None for v in a.values()),'comfy_encoded':sum(v['encoding'] is not None for v in b.values())}
        for key in sorted(set(a)&set(b)):
            x,y=a[key],b[key]
            if x['shape']!=y['shape']:
                errors.append({'component':component,'key':key,'reason':'shape','cozy':x['shape'],'comfy':y['shape']});continue
            if x['encoding']!=y['encoding']:
                errors.append({'component':component,'key':key,'reason':'encoding_or_coverage','cozy':x['encoding'],'comfy':y['encoding']});continue
            if x['encoding'] is not None:
                if x!=y:errors.append({'component':component,'key':key,'reason':'encoded_parts_or_logical_dtype','cozy':x,'comfy':y})
            elif component=='vae':
                # F32 carrying exact BF16 values can match; this hashes actual values,
                # not a rounded conversion. Compute dtype is separately fixed below.
                if x['float32_sha256']!=y['float32_sha256']:
                    errors.append({'component':component,'key':key,'reason':'vae_exact_float32_values','cozy':x,'comfy':y})
            elif x['dtype']!=y['dtype'] or x['sha256']!=y['sha256']:
                errors.append({'component':component,'key':key,'reason':'plain_values_or_dtype','cozy':x,'comfy':y})
    vae_dtypes={x['dtype'] for x in cozy['components']['vae'].values()}
    assert len(vae_dtypes)==1 and vae_dtypes<={'F32','BF16'},vae_dtypes
    a,b=cozy['sigmas'],comfy['sigmas']
    for arr in (a,b):
        assert len(arr)==31 and arr[-1]==0 and arr[0]==1
        assert all(math.isfinite(v) and v>=0 for v in arr)
        assert all(x>y for x,y in zip(arr,arr[1:]))
    natural=all(math.isclose(x,y,rel_tol=1e-5,abs_tol=2e-5) for x,y in zip(a,b,strict=True))
    return {'matched':not errors,'tensorfs_checkpoint':EXPECTED,'repository_commit':'sha256:ce2918d01f2132226405f2f5aa8c702507591b0778abfc00cc03b0c1f8da14f8',
            'comfy_commit':COMFY,'torch':cozy['torch'],'torch_git':cozy['torch_git'],
            'counts':counts,'ignored_unexecuted':ignored,'mismatches':errors,
            'comfy_files':comfy['files'],'cozy_sigmas':a,'natural_comfy_sigmas':b,
            'natural_sigmas_match':natural,'natural_sigma_max_abs_difference':max(abs(x-y) for x,y in zip(a,b,strict=True)),
            'comfy_scheduler':'simple' if natural else 'manual-cozy-observed',
            'vae_compute_dtype':next(iter(vae_dtypes)),
            'comfy_vae_flag':'--fp32-vae' if vae_dtypes=={'F32'} else '--bf16-vae',
            'comfy_launch_precision_flags':['--bf16-unet','--bf16-text-enc','--fp32-vae' if vae_dtypes=={'F32'} else '--bf16-vae'],
            'scope':'Exact canonical executed-weight/INT8representation and VAEvalue parity; real scheduler observations. GPU execution not yet proven.',
            'next_if_mismatch':'Retain this failing receipt. Use existing TensorFS decode=False export + qwen_common_files.py format-only normalization, then recreate the Comfy receipt and rerun this same comparison before timing.'}


if __name__=='__main__':
    a,b,out=map(Path,sys.argv[1:]);result=compare(json.loads(a.read_text()),json.loads(b.read_text()))
    result['receipt_sha256']={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in (a,b)}
    out.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:v for k,v in result.items() if k not in ('cozy_sigmas','natural_comfy_sigmas','mismatches')},indent=2))
    if not result['matched']:raise SystemExit(f"Qwen weights/representations do not match: {len(result['mismatches'])} differences retained in {out}")
