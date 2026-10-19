
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
const S=(n,p,req)=>({name:n,input_schema:{type:'object',properties:p,required:req}});
const P=(t,x={})=>({type:t,...x});
const sc=S('shell_command',{command:P('string'),timeout:P('number')},['command']);
for (const inp of [{cmd:'npm i', timeout:'60000'},{command:'npm i', timeout_ms:'60000'},{cmd:'npm i', timeoutMs:'60000'}]) {
  const r=repairToolInput({schema:sc,input:inp});
  console.log(JSON.stringify(inp),'->',JSON.stringify(r.input),'rules:',r.repairs.map(x=>x.rule).join(','));
}
