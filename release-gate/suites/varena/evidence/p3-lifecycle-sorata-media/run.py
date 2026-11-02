from pathlib import Path
import hashlib,json,os,subprocess,sys
from PIL import Image,ImageDraw

here=Path(__file__).resolve().parent
base=Path('/home/fidika/cozy_v2/outputs/varena-v2-p0-20261031')
attempt=base/'p3-lifecycle-sparse-leg16-plan/attempt-002'
control=attempt/'control-sequence-absolute/model-media/25482-video.mp4'
candidate=attempt/'candidate-executed/model-media/25499-video.mp4'
files=[control,candidate,control.with_name('25482-latents'),candidate.with_name('25499-latents')]
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
expected=['cbefadff7ea0081b801bba404d3d1b9fe1de0ec67cfcf4e3f5a9bde1c42bb762','2b526b9959cb54fc414110094f1dc70b940142efc01d70f029a0d8d2c1970af8','e5cdb486a712de57ce75dadc7d99760659c1f62ded044286e682f889e8429644','b0dcb3c11b03993b417265fd70229655ccf96f64cb9506877d830672b5e28f83']
assert [sha(p) for p in files]==expected
invocations=[]
def run(argv,label):
    invocations.append({'label':label,'argv':argv})
    with (here/(label+'.stdout')).open('w') as out,(here/(label+'.stderr')).open('w') as err:
        subprocess.run(argv,stdout=out,stderr=err,check=True)
run([sys.executable,str(base/'h3-results/compare-quality.py'),str(control),str(candidate),str(here/'full-video')],'compare-video')
run([sys.executable,'/home/fidika/cozy_v2/outputs/xs138-p2-tail/full-h3/audit/compare-latents.py',str(files[2]),str(files[3]),str(here/'latents.json')],'compare-latents')
report=json.loads((here/'full-video/comparison.json').read_text())
stream=next(s for s in report['baseline_probe']['streams'] if s['codec_type']=='video')
count=int(stream['nb_read_frames']); assert count==362
frames=[round(i*(count-1)/7) for i in range(8)]
for label,path in [('control',control),('candidate',candidate)]:
    directory=here/label;directory.mkdir(exist_ok=False)
    select='+'.join(f'eq(n\\,{i})' for i in frames)
    run(['ffmpeg','-v','error','-threads','1','-i',str(path),'-vf',f'select={select}',
         '-filter_threads','1','-fps_mode','passthrough','-threads','1',str(directory/'frame-%02d.png')],label+'-frames')
    assert len(list(directory.glob('frame-*.png')))==8
for group in range(2):
    sheet=Image.new('RGB',(1360,4*412),(238,238,238));draw=ImageDraw.Draw(sheet)
    for row in range(4):
        index=group*4+row
        for col,label in enumerate(('control','candidate')):
            im=Image.open(here/label/f'frame-{index+1:02d}.png').convert('RGB')
            assert im.size==(1344,768)
            im.thumbnail((672,384),Image.Resampling.LANCZOS)
            x=8+680*col;y=row*412
            draw.text((x,y+4),f'{label} run{25482 if col==0 else 25499} | frame {frames[index]} | {frames[index]/24:.3f}s',fill=(0,0,0))
            sheet.paste(im,(x,y+24))
    sheet.save(here/f'matched-frames-{group+1}.webp',lossless=True)
latents=json.loads((here/'latents.json').read_text())
summary={'files':[{'path':str(p),'sha256':sha(p),'bytes':p.stat().st_size} for p in files],
         'frames':frames,'frame_times_s':[n/24 for n in frames],'video':{k:v for k,v in report.items() if not k.endswith('_probe')},
         'video_format':{k:stream.get(k) for k in ('width','height','nb_read_frames','r_frame_rate','duration','pix_fmt')},
         'latents':{k:v for k,v in latents.items() if k not in ('metadata','per_latent_time')},
         'affinity':sorted(os.sched_getaffinity(0)),'nice':os.nice(0),'ffmpeg_threads':1,'invocations':invocations,
         'scope':'eight matched sampled frames; full decoded numerical video/audio and final video latents; visual inspection follows, no full-motion or audio-listening assessment'}
(here/'numeric-review.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({k:summary[k] for k in ('frames','video_format','latents')},indent=2))
print(report['video_summaries']);print(report['audio'])
