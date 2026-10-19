
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
function t(name, schema, input, note){
  try {
    const r=repairToolInput({schema, input});
    console.log('---',note,'\n  out:',JSON.stringify(r.input),' rules:',r.repairs.map(x=>x.rule).join(',')||'-', r.correction?' REJECT':'');
  } catch(e) { console.log('---',note,'\n  THROWS:', e.constructor.name, e.message); }
}
t('x',{name:'x',input_schema:{type:'object'}},{a:1},'schema with NO properties key');
t('x',{name:'x',input_schema:{type:'object',properties:{a:{type:'string'}}}},{a:'1'},'schema with properties but NO required');
t('x',{name:'x',input_schema:{type:'object',properties:{a:{type:'string'}},required:[]}},{a:'1'},'schema with empty required');
t('x',{name:'x',input_schema:{type:'object',properties:{},required:[],additionalProperties:true}},{a:1},'free-form MCP schema');
