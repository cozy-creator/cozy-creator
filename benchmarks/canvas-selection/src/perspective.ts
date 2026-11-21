/** Presentation-only plane tilt. Picking intersects the same projected z=0 plane. */
export function createPerspective(host:HTMLElement,width:number,height:number){
 let matrix=new DOMMatrix(),active=false;
 const project=(x:number,y:number)=>{const p=matrix.transformPoint(new DOMPoint(x,y,0,1));return{x:p.x/p.w,y:p.y/p.w}};
 const unproject=(x:number,y:number)=>{if(!active)return{x,y};const a=matrix.m11-x*matrix.m14,b=matrix.m21-x*matrix.m24,c=x*matrix.m44-matrix.m41,d=matrix.m12-y*matrix.m14,e=matrix.m22-y*matrix.m24,f=y*matrix.m44-matrix.m42,det=a*e-b*d;if(Math.abs(det)<1e-9)throw new Error('Canvas plane is edge-on');return{x:(c*e-b*f)/det,y:(a*f-c*d)/det}};
 return{project,unproject,set(x:number,y:number){if(Math.abs(x)>60||Math.abs(y)>60)throw new Error('Tilt research demo is limited to ±60 degrees');active=!!(x||y);const p=new DOMMatrix();p.m34=-1/1400;matrix=new DOMMatrix().translate(width/2,height/2).multiply(p).rotate(x,y,0).translate(-width/2,-height/2);host.style.transformOrigin='0 0';host.style.transform=active?matrix.toString():'none';return{active,x,y,matrix:[...matrix.toFloat64Array()]}}}
}
