
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
const S=(n,p,req,ex={})=>({name:n,input_schema:{type:'object',properties:p,required:req,...ex}});
const P=(t,x={})=>({type:t,...x});
const read=S('read_file',{file_path:P('string'),paths:P('array',{items:P('string')})},['file_path'],{requiredAlternatives:{file_path:['paths']}});
function t(schema,input,note){const r=repairToolInput({schema,input});console.log('---',note,'\n  out:',JSON.stringify(r.input),' rules:',r.repairs.map(x=>x.rule).join(',')||'-', r.correction?' REJECT':'');}
t(read,{include:'["a.ts","b.ts"]'},'JSON-string array under an ALIAS key (2-pass rescue)');
t(read,{include:'a.ts'},'bare string under alias for an array field');
t(S('grep',{pattern:P('string'),glob:P('string')},['pattern']),{pattern:'x',glob:['a','b']},'array given for a string field (no coercion exists)');
t(S('x',{a:P('string')},['a']),{a:'x',b:undefined},'undefined value (JSON cannot carry it, but null)');
t(S('x',{a:P('string')},['a']),{a:'x',b:{}}, 'empty object placeholder for an unknown key');
