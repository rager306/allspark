
const {reorganizeIfNeeded, parseCategories, categorySlug} = require('/tmp/cc_verify/reorg.cjs');
const fsmod=require('fs'), path=require('path');
function rt(dir){
  return {fs:{
    exists: async ({path:p})=>fsmod.existsSync(p),
    read: async ({path:p})=>fsmod.readFileSync(p,'utf8'),
    write: async ({path:p,content})=>{fsmod.mkdirSync(path.dirname(p),{recursive:true});fsmod.writeFileSync(p,content);},
    mkdir: async ({path:p})=>{try{fsmod.mkdirSync(p,{recursive:false});}catch(e){if(e.code!=='EEXIST')throw e;}},   // NON-recursive, like the real one likely
    readdir: async ({path:p})=>fsmod.readdirSync(p),
    stat: async ({path:p})=>{const s=fsmod.statSync(p);return {isDirectory:s.isDirectory(),mtimeMs:s.mtimeMs};},
  }};
}
(async()=>{
  const dir='/tmp/cc_verify/proj/.commandcode/taste';
  fsmod.rmSync('/tmp/cc_verify/proj',{recursive:true,force:true});
  fsmod.mkdirSync(dir,{recursive:true});
  // A: overlong heading (the ENAMETOOLONG / ENOENT class from issues #639, #975, #962)
  const longName = 'preference-'+'a'.repeat(280);
  const md = `# Taste (Continuously Learned by [CommandCode][cmd])\n\n[cmd]: https://commandcode.ai/\n\n# ${longName}\n` +
    Array.from({length:6},(_,i)=>`- Rule ${i}. Confidence: 0.9`).join('\n') + '\n';
  fsmod.writeFileSync(dir+'/taste.md', md);
  const notices=[];
  try { await reorganizeIfNeeded({runtime:rt(dir), dir, emit:n=>notices.push(n.message)}); console.log('A: no error; notices=',notices); }
  catch(e){ console.log('A: THROWS', e.code||e.name, String(e.message).slice(0,120)); }
  console.log('   root intact?', fsmod.readFileSync(dir+'/taste.md','utf8').includes('Rule 5'));
  console.log('   dirs:', fsmod.readdirSync(dir));

  // B: heading with a slash -> nested dirs
  fsmod.rmSync('/tmp/cc_verify/proj',{recursive:true,force:true}); fsmod.mkdirSync(dir,{recursive:true});
  fsmod.writeFileSync(dir+'/taste.md', `# css/styling\n` + Array.from({length:6},(_,i)=>`- Rule ${i}. Confidence: 0.9`).join('\n')+'\n');
  const n2=[];
  try { await reorganizeIfNeeded({runtime:rt(dir), dir, emit:n=>n2.push(n.message)}); console.log('B: notices=',n2); } catch(e){ console.log('B: THROWS',e.code); }
  console.log('   dirs:', fsmod.readdirSync(dir));

  // C: slug collision — two categories mapping to the same slug
  fsmod.rmSync('/tmp/cc_verify/proj',{recursive:true,force:true}); fsmod.mkdirSync(dir,{recursive:true});
  const six = (tag)=>Array.from({length:6},(_,i)=>`- ${tag} rule ${i}. Confidence: 0.9`).join('\n');
  fsmod.writeFileSync(dir+'/taste.md', `# Workflow A\n${six('A')}\n\n# workflow-a\n${six('B')}\n`);
  const n3=[];
  await reorganizeIfNeeded({runtime:rt(dir), dir, emit:n=>n3.push(n.message)});
  console.log('C: notices=', n3);
  console.log('   workflow-a/taste.md has B rules?', fsmod.readFileSync(dir+'/workflow-a/taste.md','utf8').includes('B rule 0'), '| has A rules?', fsmod.readFileSync(dir+'/workflow-a/taste.md','utf8').includes('A rule 0'));
  console.log('   root:', JSON.stringify(fsmod.readFileSync(dir+'/taste.md','utf8')));
})();
