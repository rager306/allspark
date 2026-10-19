
// single-pass simulation: no second sweep
const m = require('/tmp/cc_verify/repair.cjs');
const S=(n,p,req)=>({name:n,input_schema:{type:'object',properties:p,required:req}});
const P=(t,x={})=>({type:t,...x});
const sc=S('shell_command',{command:P('string'),timeout:P('number')},['command']);
const {runCatalogue} = m;
// monkey: call runCatalogue directly is not exported with notes access; simulate by
// calling repairToolInput on the already-aliased-but-uncoerced intermediate:
const mid = {command:'npm i', timeout:'60000'};
const r = m.repairToolInput({schema:sc, input: mid});
console.log('if pass1 produced {timeout:"60000"} and we STOPPED, we would ship:', JSON.stringify(mid));
console.log('pass2 fixes it to:', JSON.stringify(r.input));
