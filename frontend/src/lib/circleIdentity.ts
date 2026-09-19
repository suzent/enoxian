export const CIRCLE_FAMILIES = ['halo', 'field', 'orb', 'seal'] as const
export type CircleFamily = typeof CIRCLE_FAMILIES[number]
export function hash(id: string) { let h=2166136261; for(const c of id){h^=c.charCodeAt(0);h=Math.imul(h,16777619)} return h>>>0 }
export function circleFamily(id: string): CircleFamily {
 return CIRCLE_FAMILIES[hash(id)%4]
}
export function parameters(id: string) {const h=hash('circle-'+id);return {sides:[4,6,8][(h>>>2)%3],inner:[3,4,6][(h>>>4)%3],frame:(h>>>8)%3,core:(h>>>12)%3,phase:((h>>>16)%8)*Math.PI/8,links:(h>>>21)%2,offset:(h>>>23)%2}}
export function drawCircleMark(ctx: CanvasRenderingContext2D,x: number,y: number,r: number,id: string,family: CircleFamily,color: string,trace=1,turn=0){
 const p=parameters(id);
 ctx.save();ctx.translate(x,y);ctx.strokeStyle=color;ctx.fillStyle=ctx.strokeStyle;ctx.lineWidth=Math.max(.8,r/65);ctx.lineJoin='round';
 if(family!=='seal'){
  const bayer=[0,8,2,10,12,4,14,6,3,11,1,9,15,7,13,5];
  const phase=p.phase+turn;const grain=2/64;
  for(let iy=0;iy<64;iy++)for(let ix=0;ix<64;ix++){
   const u=(ix-31.5)*grain,v=(iy-31.5)*grain,rr=Math.hypot(u,v),a=Math.atan2(v,u);let density=0;
   if(family==='halo'){
    const radius=.64+(p.core-1)*.05;
    density=Math.exp(-Math.pow((rr-radius)/.095,2))*(.55+.32*Math.cos(a-phase));
    if(p.frame===1)density+=Math.exp(-Math.pow((rr-.88)/.025,2))*.6;
    if(p.frame===2)density+=Math.exp(-Math.pow((rr-.35)/.04,2))*.7;
    const gap=Math.abs(Math.atan2(Math.sin(a-phase),Math.cos(a-phase)));
    if(gap<.16+p.offset*.1&&rr>.5)density*=.15;
    if((a+Math.PI)/(Math.PI*2)>trace)density*=.1;
   }else if(family==='field'){
    const lobes=p.inner;const outline=.72+.11*Math.cos(a*lobes+phase)+.04*Math.sin(a*2-phase);
    const xx=u-Math.cos(phase)*.15,yy=v-Math.sin(phase)*.15;
    density=Math.max(0,1-rr/outline)*(.5+.4*Math.sin(xx*5+yy*3+phase)**2);
    density+=.2*Math.exp(-(xx*xx+yy*yy)/.08);density*=trace;
   }else if(rr<.81){
    const nx=u/.81,ny=v/.81,nz=Math.sqrt(Math.max(0,1-nx*nx-ny*ny));
    const lx=Math.cos(phase)*.65,ly=Math.sin(phase)*.65,lz=.85;
    const light=Math.max(0,(nx*lx+ny*ly+nz*lz)/Math.hypot(lx,ly,lz));
    density=(.08+.88*(1-light)**1.3)*trace;
    if(p.core===1&&Math.abs(v*.7+u*.5)<.055)density*=.3;
    if(p.core===2&&Math.abs(v*.55-u*.7)<.035)density=Math.min(1,density+.35);
   }
   if(density>(bayer[(iy%4)*4+ix%4]+.5)/16)ctx.fillRect(u*r,v*r,Math.max(.6,r*grain*.82),Math.max(.6,r*grain*.82));
  }
  if(family==='halo'){ctx.beginPath();ctx.arc(0,0,r*.23,0,Math.PI*2*trace);ctx.stroke();ctx.beginPath();ctx.arc(Math.cos(phase)*r*.88,Math.sin(phase)*r*.88,Math.max(1.4,r*.035),0,Math.PI*2);ctx.fill()}
  ctx.restore();return;
 }
 const polar=(a: number,rr: number): [number, number]=>[Math.cos(a)*rr*r,Math.sin(a)*rr*r];const polygon=(n: number,rr: number,a: number)=>Array.from({length:n},(_,i)=>polar(a+i*Math.PI*2/n,rr));
 const path=(pts: [number, number][],closed=true)=>{ctx.beginPath();pts.forEach((q,i)=>i?ctx.lineTo(...q):ctx.moveTo(...q));if(closed)ctx.closePath();ctx.setLineDash(trace<1?[r*8*trace,r*10]:[]);ctx.stroke();ctx.setLineDash([])};
 const ring=(rr: number)=>{ctx.beginPath();ctx.arc(0,0,r*rr,-Math.PI/2,-Math.PI/2+Math.PI*2*trace);ctx.stroke()};
 const a=p.phase+turn,corners=polygon(p.sides,.92,a);
 if(p.frame===2){ring(.92);path(polygon(p.sides,.8,a))}else{path(corners);if(p.frame===1){ctx.globalAlpha=.4;path(polygon(p.sides,.79,a));ctx.globalAlpha=1}}
 path(polygon(p.inner,.52,-p.phase-turn*.73+(p.offset?Math.PI/p.inner:0)));
 if(p.links){ctx.globalAlpha=.45;for(let i=0;i<p.sides;i+=2)path([corners[i],polar(a+i*Math.PI*2/p.sides,.25)],false);ctx.globalAlpha=1}
 if(p.core===1)ring(.25);
 ctx.globalAlpha=trace;
 const step=Math.max(1.2,r/28);
 for(let yy=-r*.32;yy<r*.32;yy+=step)for(let xx=-r*.32;xx<r*.32;xx+=step){const d=p.core===2?Math.abs(xx)+Math.abs(yy):Math.hypot(xx,yy);if(d<r*.3&&((Math.floor(xx/step)*7+Math.floor(yy/step)*13)%5+5)%5<2)ctx.fillRect(xx,yy,Math.max(.65,r/100),Math.max(.65,r/100))}
 corners.forEach(q=>{ctx.beginPath();ctx.arc(...q,Math.max(1,r/45),0,Math.PI*2);ctx.fill()});ctx.beginPath();ctx.arc(0,0,Math.max(1,r/36),0,Math.PI*2);ctx.fill();ctx.restore();
}
