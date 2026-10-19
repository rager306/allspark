
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
function t(name, schema, input, note){
  const r=repairToolInput({schema, input});
  console.log('---',note||name,'\n  in :',JSON.stringify(input),'\n  out:',JSON.stringify(r.input),'\n  rules:',r.repairs.map(x=>x.rule).join(',')||'-', r.correction?'\n  REJECT '+r.correction.split('\n').slice(0,2).join(' | '):'');
}
const readSchema={name:'read_file',input_schema:{type:'object',properties:{file_path:{type:'string'},paths:{type:'array',items:{type:'string'}},offset:{type:'number'},limit:{type:'number'}},required:['file_path']},requiredAlternatives:{file_path:['paths']}};
t('read_file', readSchema, {include:['a','b']}, 'read_file include->paths + requiredAlternatives honoured?');
t('read_file', readSchema, {paths:['a','b']}, 'read_file paths only');
t('read_file', readSchema, {file_path:'/a', paths:['b']}, 'read_file BOTH file_path and paths');
