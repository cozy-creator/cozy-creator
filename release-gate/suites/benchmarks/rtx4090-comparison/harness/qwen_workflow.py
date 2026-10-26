"""Existing Comfy nodes, actual matched Qwen2.1 file/sigma receipts, no model substitution."""
import math
from pathlib import Path


def node(kind,**inputs):return {'class_type':kind,'inputs':inputs}


def qwen21(job,match):
    assert match['matched'] is True
    assert job['size']==1024 and job['steps']==30 and job.get('batch',1)==1
    assert not job.get('reference_images') and not job.get('lora')
    assert float(job['guidance'])==1
    files=match['comfy_files']
    graph={
        '1':node('UNETLoader',unet_name=Path(files['transformer']['path']).name,weight_dtype='default'),
        '2':node('QwenImage21Cache',model=['1',0],device='auto',dtype='default'),
        '3':node('CLIPLoader',clip_name=Path(files['text_encoder']['path']).name,type='qwen_image',device='default'),
        '4':node('VAELoader',vae_name=Path(files['vae']['path']).name),
        '5':node('TextEncodeQwenImage21',clip=['3',0],prompt=job['prompt'],negative_prompt='',resolution=1024),
        '8':node('VAEDecode',samples=['7',0],vae=['4',0]),
        '9':node('SaveImage',images=['8',0],filename_prefix=f"bench/{job['label']}"),
    }
    common={'model':['2',0],'cfg':1.0,'positive':['5',0],'negative':['5',1],'latent_image':['5',2]}
    if match['comfy_scheduler']=='simple':
        graph['7']=node('KSampler',**common,seed=job['seed'],steps=30,sampler_name='euler',scheduler='simple',denoise=1.0)
    else:
        assert match['comfy_scheduler']=='manual-cozy-observed'
        sigmas=match['cozy_sigmas'];assert len(sigmas)==31 and sigmas[0]==1 and sigmas[-1]==0 and all(math.isfinite(v) for v in sigmas)
        graph['10']=node('ManualSigmas',sigmas=', '.join(format(x,'.17f') for x in sigmas))
        graph['11']=node('KSamplerSelect',sampler_name='euler')
        graph['7']=node('SamplerCustom',**common,add_noise=True,noise_seed=job['seed'],sampler=['11',0],sigmas=['10',0])
    return graph
