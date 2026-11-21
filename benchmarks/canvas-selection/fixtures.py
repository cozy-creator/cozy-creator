"""Generate public photographic, distinct-resource fixtures; outputs are never committed."""
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
import argparse, hashlib, json, random, urllib.request, subprocess
from PIL import Image, ImageDraw
p=argparse.ArgumentParser(); p.add_argument('--out',type=Path,required=True); p.add_argument('--count',type=int,default=5000); a=p.parse_args()
a.out.mkdir(parents=True,exist_ok=True)
ids=['1500534623283-312aade485b7','1470770841072-f978cf4d019e','1464822759023-fed622ff2c3b','1441974231531-c6227db76b6e','1472396961693-142e6e269027','1519681393784-d120267933ba','1518837695005-2083093ee35b','1426604966848-d7adac402bff']
sources=[]; records=[]
for i,ident in enumerate(ids):
 url=f'https://images.unsplash.com/photo-{ident}?w=1024&q=85&fit=crop'; f=a.out/f'source-{i}.jpg'
 if not f.exists(): f.write_bytes(urllib.request.urlopen(urllib.request.Request(url,headers={'User-Agent':'Creator canvas benchmark fixture generator'}),timeout=30).read())
 im=Image.open(f).convert('RGB'); sources.append(im); records.append({'url':url,'sha256':hashlib.sha256(f.read_bytes()).hexdigest(),'size':list(im.size)})
tiers=[32,64,128,256,512]
def one(i):
 folder=a.out/'cards'/str(i); folder.mkdir(parents=True,exist_ok=True)
 if (folder/'512.webp').exists(): return
 im=sources[i%len(sources)]; w,h=im.size; r=random.Random(271828+i); cw=int(min(w,h*16/9)*r.uniform(.55,1)); ch=int(cw*9/16)
 x=r.randrange(max(1,w-cw+1)); y=r.randrange(max(1,h-ch+1)); crop=im.crop((x,y,x+cw,y+ch))
 for tier in tiers:
  dst=crop.resize((tier,round(tier*9/16)),Image.Resampling.LANCZOS)
  ImageDraw.Draw(dst).line([(0,dst.height-1),(dst.width-1,dst.height-1)],fill=((i*37)%256,(i*83)%256,(i*131)%256))
  dst.save(folder/f'{tier}.webp','WEBP',quality=82,method=0)
with ThreadPoolExecutor(max_workers=8) as ex:
 for i,_ in enumerate(ex.map(one,range(a.count)),1):
  if i%500==0: print(f'{i}/{a.count} photographic assets generated',flush=True)
manifest={'count':a.count,'tiers':tiers,'aspect':[16,9],'seed':271828,'distinctResources':True,'sources':records,'policy':'Public landscapes with seeded distinct crops and one identifying pixel strip; identical assets across renderers.'}
(a.out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n'); print(json.dumps({'count':a.count,'out':str(a.out),'tiers':tiers}),flush=True)

# Complete offline browser fixture set used by the interaction and hardware probes.
(a.out/'probe.html').write_text('<!doctype html><html><head><link rel="icon" href="data:,"><title>GPU capability probe</title></head><body>GPU capability probe</body></html>')
duck_url='https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/Duck/glTF-Binary/Duck.glb'
duck=a.out/'duck.glb'
if not duck.exists(): duck.write_bytes(urllib.request.urlopen(duck_url,timeout=30).read())
video=a.out/'motion.mp4'
video_args=['ffmpeg','-hide_banner','-loglevel','error','-f','lavfi','-i','testsrc2=size=640x360:rate=30','-t','4','-c:v','libx264','-preset','veryfast','-pix_fmt','yuv420p','-movflags','+faststart',str(video)]
if not video.exists(): subprocess.run(video_args,check=True)
manifest['model']={'url':duck_url,'sha256':hashlib.sha256(duck.read_bytes()).hexdigest(),'attribution':'Khronos glTF Sample Assets Duck; see upstream Models/Duck/README.md for model licensing.'}
manifest['video']={'recipe':video_args[:-1]+['motion.mp4'],'sha256':hashlib.sha256(video.read_bytes()).hexdigest()}
(a.out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')

# Assets for the local interactive review route.
import shutil
demo=a.out/'demo';demo.mkdir(exist_ok=True)
for size in [256,512]:
 poster=demo/f'video{size}.webp'
 if not poster.exists():subprocess.run(['ffmpeg','-hide_banner','-loglevel','error','-ss','0.7','-i',str(video),'-frames:v','1','-vf',f'scale={size}:{round(size*9/16)}','-c:v','libwebp',str(poster)],check=True)
logo=Path(__file__).resolve().parents[2]/'web/app/public/assets/apple-touch-icon.png'
if logo.exists():shutil.copyfile(logo,demo/'cozy-icon.png')
