"""Coarse displayed-photograph gate; detects blank output, not full visual fidelity."""
import argparse,json,math,sys
from pathlib import Path
from PIL import Image
p=argparse.ArgumentParser();p.add_argument('--assert',dest='assert_gate',action='store_true');p.add_argument('--verbose',action='store_true');a=p.parse_args();s=json.load(sys.stdin)
im=Image.open(s['screenshot']).convert('RGB');n=s['n'];cols=math.ceil(math.sqrt(n*1.4));cam=s['camera'];dpr=s.get('dpr',1);pool=s.get('pool',n);tier=s['tier'];root=Path(s['fixtures']);bg=[(17,24,32),(0,0,0)];samples=[]
def distance(x,y):return sum((a-b)**2 for a,b in zip(x,y))**.5
def mean_patch(image,x,y,r=1):
 x,y=round(x),round(y);points=[image.getpixel((xx,yy)) for xx in range(max(0,x-r),min(image.width,x+r+1)) for yy in range(max(0,y-r),min(image.height,y+r+1))]
 return tuple(sum(v[c] for v in points)/len(points) for c in range(3)) if points else (0,0,0)
candidates=[]
for i in range(n):
 x=(i%cols*256+120)*cam['zoom']+cam['x'];y=(i//cols*216+95)*cam['zoom']+cam['y']
 if 2<x<s['width']-2 and 2<y<s['height']-2:candidates.append(i)
for i in candidates[::max(1,len(candidates)//128)]:
 x=(i%cols*256+120)*cam['zoom']+cam['x'];y=(i//cols*216+95)*cam['zoom']+cam['y']
 if not(2<x<s['width']-2 and 2<y<s['height']-2):continue
 source=Image.open(root/'cards'/str(i%pool)/f'{tier}.webp').convert('RGB');expected=mean_patch(source,source.width/2,source.height/2)
 if min(distance(expected,b) for b in bg)<75:continue
 actual=mean_patch(im,x*dpr,y*dpr);contrast=min(distance(actual,b) for b in bg);delta=sum(abs(a-b) for a,b in zip(actual,expected))/3
 samples.append({'id':str(i),'x':x,'y':y,'expected':expected,'actual':actual,'contrast':contrast,'meanChannelError':delta,'present':contrast>30,'roughColorMatch':delta<60})
coverage=sum(v['present'] for v in samples)/len(samples) if samples else 0;color=sum(v['roughColorMatch'] for v in samples)/len(samples) if samples else 0
result={'pass':len(samples)>=min(10,n) and coverage>=.95 and color>=.85,'sampleCount':len(samples),'presentFraction':coverage,'roughColorMatchFraction':color,'meaning':'Coarse photo presence + broad source-color check; excludes dark source samples; not full pixel parity','samples':samples if a.verbose else [v for v in samples if not v['present'] or not v['roughColorMatch']][:4]}
print(json.dumps(result));sys.exit(1 if a.assert_gate and not result['pass'] else 0)
