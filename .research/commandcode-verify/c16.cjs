
const {reorganizeIfNeeded} = require('/tmp/cc_verify/reorg.cjs');
const fsmod=require('fs'), path=require('path');
function rt(){return {fs:{
  exists: async ({path:p})=>fsmod.existsSync(p),
  read: async ({path:p})=>fsmod.readFileSync(p,'utf8'),
  write: async ({path:p,content})=>{fsmod.mkdirSync(path.dirname(p),{recursive:true});fsmod.writeFileSync(p,content);},
  mkdir: async ({path:p})=>{try{fsmod.mkdirSync(p,{recursive:false});}catch(e){if(e.code!=='EEXIST')throw e;}},
  readdir: async ({path:p})=>fsmod.readdirSync(p),
  stat: async ({path:p})=>{const s=fsmod.statSync(p);return{isDirectory:s.isDirectory(),mtimeMs:s.mtimeMs};},
}};}
(async()=>{
  const dir='/tmp/cc_verify/proj2/.commandcode/taste';
  fsmod.rmSync('/tmp/cc_verify/proj2',{recursive:true,force:true}); fsmod.mkdirSync(dir,{recursive:true});
  const HDR='# Taste (Continuously Learned by [CommandCode][cmd])\n\n[cmd]: https://commandcode.ai/\n';
  fsmod.writeFileSync(dir+'/taste.md', HDR+'\n'+Array.from({length:6},(_,i)=>`- Rule ${i}. Confidence: 0.9`).join('\n')+'\n');
  const n=[];
  await reorganizeIfNeeded({runtime:rt(),dir,emit:x=>n.push(x.message)});
  console.log('notices:',n);
  console.log('dirs:',fsmod.readdirSync(dir));
  console.log('root now:',JSON.stringify(fsmod.readFileSync(dir+'/taste.md','utf8')));
  for (const d of fsmod.readdirSync(dir)) if(d!=='taste.md') console.log(d+'/taste.md:',JSON.stringify(fsmod.readFileSync(dir+'/'+d+'/taste.md','utf8')));
})();
