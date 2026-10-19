
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
function t(name, schema, input, note){
  const r=repairToolInput({schema, input});
  console.log('---',note,'\n  out:',JSON.stringify(r.input),'\n  rules:',r.repairs.map(x=>x.rule).join(','),'\n  hints:',JSON.stringify(r.repairs.map(x=>x.fix)), r.correction?'\n  REJECT':'');
}
const S=(n,p,req,ex={})=>({name:n,input_schema:{type:'object',properties:p,required:req,...ex}});
const P=(t,x={})=>({type:t,...x});
t('shell_command', S('shell_command',{command:P('string'),timeout:P('number')},['command']), {command:'npm i', timeout:'60000'}, 'alias + string number: two-pass sweep rescues it?');
t('grep', S('grep',{pattern:P('string'),glob:P('string'),type:P('string')},['pattern']), {pattern:'x', include:'*.ts'}, 'grep include (runtime reads it, schema drops it) -> SILENT LOSS');
t('vision', S('vision',{question:P('string'),image_index:P('number')},['question']), {question:'q', image:'/tmp/shot.png'}, 'vision.image(path)->image_index(number): type confusion');
