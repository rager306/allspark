
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
const agentSchema={name:'agent',input_schema:{type:'object',properties:{description:{type:'string'},subagent_type:{type:'string',enum:['general','explore','plan','review']},prompt:{type:'string'},model:{type:'string'}},required:['description','subagent_type','prompt']}};
const r=repairToolInput({schema:agentSchema,input:{messages:[{content:'Find auth middleware.\nDepth: quick\nReturn the path.'}]}});
console.log(JSON.stringify(r.input));
console.log(r.correction);
