
const {coerceToolInput, resolveToolNameAlias} = require('/tmp/cc_verify/coerce.cjs');
const S = (name, props, required) => ({name, input_schema:{type:'object', properties:props, required}});
const P=(t,x={})=>({type:t,...x});
const schemas={
  edit_file:S('edit_file',{file_path:P('string'),old_string:P('string'),new_string:P('string')},['file_path','old_string','new_string']),
  todo_write:S('todo_write',{todos:P('array',{items:P('object')})},['todos']),
  grep:S('grep',{pattern:P('string')},['pattern']),
  ask_user_question:S('ask_user_question',{questions:P('array'),header:P('string')},['questions']),
};
function c(raw, toolName){
  const r = coerceToolInput({raw, toolName, schema:schemas[toolName]});
  console.log('---', toolName, 'raw=', JSON.stringify(raw));
  console.log('  ->', JSON.stringify(r.value), r.coercion?JSON.stringify(r.coercion):'');
}
c([{file_path:'/a', old_string:'x', new_string:'y'}], 'edit_file');
c([], 'edit_file');
c(null, 'edit_file');
c('{"file_path":"/a","old_string":"x","new_string":"y"}', 'edit_file');
c('not json at all', 'grep');           // 1 required string -> wrap
c('not json at all', 'edit_file');      // 3 required -> no wrap
c('"just a string"', 'grep');           // JSON string
c(42, 'grep');                          // number root
console.log('--- alias table');
for (const n of ['bash','Bash','bash_output','task_output','tool_search','read_multiple_files','grep','Edit']) {
  const r = resolveToolNameAlias({toolName:n, input:{block:false}});
  console.log(' ', n, '->', r.toolName, JSON.stringify(r.input), r.note?'(note)':'');
}
console.log('--- alias arg carry-over on task_output with explicit wait');
console.log(' ', JSON.stringify(resolveToolNameAlias({toolName:'task_output', input:{wait:'output'}})));
