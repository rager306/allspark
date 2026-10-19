
const {repairToolInput, resolveToolNameAlias, Rf, zf} = require('/tmp/cc_verify/repair.cjs');
const S = (name, props, required, extra={}) => ({name, input_schema:{type:'object', properties:props, required, ...extra}});
const P = (type, extra={}) => ({type, ...extra});
const schemas = {
  edit_file: S('edit_file',{file_path:P('string'),old_string:P('string'),new_string:P('string'),replace_all:P('boolean'),replacement_count:P('number')},['file_path','old_string','new_string']),
  grep: S('grep',{pattern:P('string'),path:P('string'),glob:P('string'),output_mode:P('string'),'-A':P('number'),'-n':P('boolean'),head_limit:P('number'),offset:P('number'),multiline:P('boolean'),type:P('string')},['pattern']),
  read_file: S('read_file',{file_path:P('string'),paths:P('array',{items:P('string')}),offset:P('number'),limit:P('number'),exclude:P('array',{items:P('string')}),target_directory:P('string'),default_exclude:P('boolean'),git_ignore:P('boolean')},['file_path'],{requiredAlternatives:{file_path:['paths']}}),
  shell_output: S('shell_output',{id:P('string'),timeout_ms:P('number'),from_offset:P('number'),max_chars:P('number'),block:P('boolean'),wait:P('string')},['id']),
  todo_write: S('todo_write',{todos:P('array',{items:{type:'object',properties:{content:P('string'),status:P('string'),activeForm:P('string'),id:P('string')},required:['content','status']}})},['todos']),
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
  return r;
}
module.exports={t,schemas};
