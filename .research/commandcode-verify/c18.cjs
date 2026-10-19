
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
const read={name:'read_file',input_schema:{type:'object',properties:{file_path:{type:'string'},paths:{type:'array',items:{type:'string'}}},required:['file_path']},requiredAlternatives:{file_path:['paths']}};
for (const input of [{include:'["a.ts","b.ts"]'},{include:'a.ts'},{paths:['a.ts']}]) {
  const r=repairToolInput({schema:read,input});
  console.log(JSON.stringify(input),'->',JSON.stringify(r.input), r.correction?('\n  REJECT: '+r.correction):'');
}
