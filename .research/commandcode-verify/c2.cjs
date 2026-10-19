
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
const S = (name, props, required, extra={}) => ({name, input_schema:{type:'object', properties:props, required}, ...extra});
const P = (type, extra={}) => ({type, ...extra});
const schemas = {
  read_file: S('read_file',{file_path:P('string'),paths:P('array',{items:P('string')}),offset:P('number'),limit:P('number'),exclude:P('array',{items:P('string')})},['file_path'],{requiredAlternatives:{file_path:['paths']}}),
  grep: S('grep',{pattern:P('string'),path:P('string'),'-A':P('number'),'-n':P('boolean'),multiline:P('boolean'),glob:P('string')},['pattern']),
  sleep: S('sleep',{seconds:P('number'),until:P('string'),wake_on_input:P('boolean')},[],{exactlyOneOf:[['seconds','until']]}),
  write_file: S('write_file',{file_path:P('string'),content:P('string')},['file_path','content']),
  task_create: S('task_create',{subject:P('string'),description:P('string'),activeForm:P('string')},['subject']),
  shell_command: S('shell_command',{command:P('string'),cwd:P('string'),timeout:P('number'),run_in_background:P('boolean')},['command']),
};
function t(name, input, note){
  const r = repairToolInput({schema: schemas[name], input});
  console.log('---', note||name);
  console.log('  in :', JSON.stringify(input));
  console.log('  out:', JSON.stringify(r.input));
  console.log('  rules:', r.repairs.map(x=>x.rule).join(',')||'-');
  if (r.correction) console.log('  REJECT:', r.correction.replace(/\n/g,'\n          '));
  console.log();
}
t('read_file', {include:['a.ts','b.ts']}, 'read_file include->paths WITH requiredAlternatives at schema level');
t('read_file', {file_path:'[x](/a.ts)'}, 'markdown link in path');
t('read_file', {file_path:'/a.ts', limit:'50', offset:'0'}, 'string numbers');
t('read_file', {file_path:'/a.ts', limit:null, exclude:null}, 'null fields');
t('read_file', {file_path:'/a.ts', exclude:[]}, 'empty array kept?');
t('read_file', {file_path:'/a.ts', paths:[]}, 'empty paths');
t('grep', {pattern:'x','-A':'three'}, 'non-numeric string for number -> reject');
t('grep', {'pattern?':'x'}, 'optional marker stripped');
t('grep', {pattern:'x', glob:['*.ts','*.tsx']}, 'array for string field -> NOT wrapped (type mismatch)');
t('grep', {pattern:'x', multiline:'true'}, 'string boolean');
t('grep', {pattern:'x', multiline:'yes'}, 'bad boolean string');
t('sleep', {seconds:'5', until:'2026-01-01'}, 'exactlyOneOf both provided');
t('sleep', {}, 'exactlyOneOf none');
t('write_file', {file_path:'/a.txt', content:''}, 'empty content string kept (minLength absent)');
t('task_create', {title:'do', body:'x', active_form:'doing'}, 'task aliases');
