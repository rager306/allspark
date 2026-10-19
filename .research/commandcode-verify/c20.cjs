
const {resolveToolNameAlias} = require('/tmp/cc_verify/coerce.cjs');
for (const inp of [{block:false},{block:true},{block:'false'},{block:'true'},{},null]) {
  console.log(JSON.stringify(inp), '->', JSON.stringify(resolveToolNameAlias({toolName:'task_output', input: inp}).input));
}
