
const {runTasteTool} = require('/tmp/cc_verify/run.cjs');
// in-memory fs
function makeFs(init={}){
  const files = {...init};
  return {
    exists: async ({path}) => path in files,
    read: async ({path}) => { if(!(path in files)) throw new Error('ENOENT '+path); return files[path]; },
    write: async ({path,content}) => { files[path]=content; },
    mkdir: async ({path}) => { files['DIR:'+path]=''; },
    _files: files,
  };
}
(async()=>{
  const dir='/proj/.commandcode/taste';
  const fs=makeFs({[dir+'/taste.md']:'# workflow\n- Old rule. Confidence: 0.5\n'});
  const runtime={fs};
  const notices=[];
  const emit=n=>notices.push(n.message);
  const r1=await runTasteTool({runtime,dir,name:'edit_taste_file',input:{path:'taste.md',old_text:'Old rule. Confidence: 0.5',new_text:'Old rule. Confidence: 0.9'},emit});
  console.log('edit ->',r1); console.log(' notices:',notices); console.log();
  // edit that drops a rule (delete) — is there a "removed" signal?
  notices.length=0;
  const r2=await runTasteTool({runtime,dir,name:'edit_taste_file',input:{path:'taste.md',old_text:'\n- Old rule. Confidence: 0.9',new_text:''},emit});
  console.log('delete ->',r2); console.log(' notices:',notices, '(empty = no removal signal)'); console.log();
  // path traversal
  for (const p of ['../evil.md','../../etc/passwd','sub/../taste.md','sub/deep/taste.md','./taste.md','taste.md']) {
    const r=await runTasteTool({runtime,dir,name:'read_taste_file',input:{path:p},emit});
    console.log('read', JSON.stringify(p), '->', JSON.stringify(r).slice(0,110));
  }
  // edit with old_text not found
  console.log();
  console.log('missing old_text ->', await runTasteTool({runtime,dir,name:'edit_taste_file',input:{path:'taste.md',old_text:'NOPE',new_text:'X'},emit}));
  // write to a category dir
  console.log('write cat ->', await runTasteTool({runtime,dir,name:'write_taste_file',input:{path:'cli/taste.md',content:'# cli\n- A. Confidence: 0.9\n'},emit}));
  console.log('files:', Object.keys(fs._files));
})();
