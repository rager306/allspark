
const {repairToolInput} = require('/tmp/cc_verify/repair.cjs');
function t(name, schema, input, note){
  const r=repairToolInput({schema, input});
  console.log('---',note||name,'\n  in :',JSON.stringify(input),'\n  out:',JSON.stringify(r.input),'\n  rules:',r.repairs.map(x=>x.rule).join(',')||'-', r.correction?'\n  REJECT '+r.correction.split('\n').slice(0,3).join(' | '):'');
}
// MCP-style tool with additionalProperties:true and no declared properties
t('mcp__x__y', {name:'mcp__x__y', input_schema:{type:'object', properties:{}, required:[], additionalProperties:true}}, {foo:1, bar:'baz'}, 'MCP free-form schema (additionalProperties:true, no props)');
// MCP tool with declared props + extra passthrough
t('mcp__x__z', {name:'mcp__x__z', input_schema:{type:'object', properties:{a:{type:'string'}}, required:['a'], additionalProperties:true}}, {a:'1', b:2}, 'MCP schema with extra passthrough');
// nested object not swept
t('cfg', {name:'cfg', input_schema:{type:'object', properties:{opt:{type:'object', properties:{depth:{type:'number'}}}}, required:[]}}, {opt:{depth:'3', nope:1}}, 'nested object: not swept (one-level only)');
// array-of-objects swept
t('lst', {name:'lst', input_schema:{type:'object', properties:{items:{type:'array', items:{type:'object', properties:{depth:{type:'number'}}, required:[]}}}, required:[]}}, {items:[{depth:'3', nope:1}]}, 'array-of-objects: swept');
