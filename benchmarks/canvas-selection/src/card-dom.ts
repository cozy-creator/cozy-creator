import type { Card } from './types';
export function makeCardContent(card: Card, url: string, lod: number): HTMLElement {
 const el=document.createElement('div'); el.className='bench-card'; el.dataset.benchCard=card.id;
 const header=document.createElement('div'); header.className='bench-header';
 const name=document.createElement('span'); name.className='bench-name'; const version=document.createElement('span'); version.className='bench-version'; version.textContent='v1'; header.append(name,version);
 const image=document.createElement('img'); image.className='bench-image'; image.alt=''; image.draggable=false;
 const status=document.createElement('div'); status.className='bench-status';
 el.append(header,image,status); updateCardContent(el,card,url,lod,false); return el;
}
export function updateCardContent(el: HTMLElement, card: Card, url: string, lod: number, selected: boolean) {
 el.dataset.lod=String(lod); el.classList.toggle('bench-selected',selected);
 el.style.width=`${card.width}px`; el.style.height=`${card.height}px`;
 const name=el.querySelector('.bench-name'); if(name && name.textContent!==card.label) name.textContent=card.label;
 const status=el.querySelector('.bench-status'); if(status && status.textContent!==card.status) status.textContent=card.status;
 const img=el.querySelector<HTMLImageElement>('img');
 if(img && img.getAttribute('src')!==url) { if(url) img.src=url; else img.removeAttribute('src'); }
}
