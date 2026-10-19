
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
const S=(name,props,required,extra={})=>({name,input_schema:{type:'object',properties:props,required,...extra}});
const P=(t,x={})=>({type:t,...x});
const schemas={
  task_stop:S('task_stop',{taskId:P('string')},['taskId']),
  kill_shell:S('kill_shell',{taskId:P('string')},['taskId']),
  grep:S('grep',{pattern:P('string'),path:P('string'),glob:P('string'),output_mode:P('string')},['pattern']),
  glob:S('glob',{pattern:P('string'),path:P('string'),limit:P('number'),offset:P('number')},['pattern']),
  shell_output:S('shell_output',{id:P('string'),wait:P('string'),block:P(['boolean','null']),timeout_ms:P(['number','null'])},['id']),
  write_file:S('write_file',{file_path:P('string'),content:P('string')},['file_path','content']),
  vision:S('vision',{question:P('string'),image_index:P('number')},['question']),
  agent:S('agent',{messages:P('array'),description:P('string'),subagent_type:P('string')},['messages']),
};
function t(n,i,note){
  const r=repairToolInput({schema:schemas[n],input:i});
  console.log('---',note||n,'\n  in :',JSON.stringify(i),'\n  out:',JSON.stringify(r.input),'\n  rules:',r.repairs.map(x=>x.rule).join(',')||'-', r.correction?'\n  REJECT '+r.correction.split('\n')[0]:'');
}
t('task_stop',{id:'s0abc'},'task_stop with id (runtime reads id, alias table has NO task_stop)');
t('kill_shell',{id:'s0abc'},'kill_shell with id (alias table HAS kill_shell)');
t('grep',{pattern:'x',include:['*.ts']},'grep with include (runtime reads e.include, schema+aliases have none)');
t('shell_output',{id:'s1',blocking:true},'shell_output blocking->block (alias points at DEPRECATED field)');
t('shell_output',{id:'s1',block:null},'shell_output block:null (schema allows null!)');
t('shell_output',{id:'s1',timeout:5000},'shell_output timeout->timeout_ms');
t('vision',{question:'q',image:'./shot.png'},'vision image(path string)->image_index');
t('agent',{messages:[{content:'x'}],description:'d',subagent:'explore'},'agent subagent->? (no alias)');
t('write_file',{file_path:'/a',content:['line1','line2']},'write_file content as ARRAY (string expected)');
