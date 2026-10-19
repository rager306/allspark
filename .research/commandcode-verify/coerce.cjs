
const __name=(fn,name)=>{try{Object.defineProperty(fn,'name',{value:name})}catch{};return fn};
function readString2({input,key}){const v=input?.[key];return typeof v==='string'?v:undefined}
function errorResult({error}){return{ok:false,error}}
function textResult({text}){return{ok:true,content:[{type:'text',text}]}}

const zh="tool_search", Kh="search_tools";
function readBooleanArg({input,key}){return {value:input?.[key]}}
kC={bash_output:{to:"shell_output"},task_output:{to:"shell_output",defaults:{wait:"exit"}},[zh]:{to:Kh},read_multiple_files:{to:"read_file"},bash:{to:"shell_command",gateAsCanonical:!0},Bash:{to:"shell_command",gateAsCanonical:!0}};
function blockCancelsWait(e){return!1===readBooleanArg({input:e,key:"block"}).value}function resolveToolNameAlias(e){const n=kC[e.toolName];if(void 0===n)return e;const o={...e.input};for(const[e,r]of Object.entries(n.defaults??{}))void 0!==o[e]&&null!==o[e]||"wait"===e&&blockCancelsWait(o)||(o[e]=r);return{toolName:n.to,input:o,note:`Repair note: the tool \`${e.toolName}\` is now \`${n.to}\`; this call ran as ${n.to} with your arguments carried over. Call \`${n.to}\` directly next time.`}};
function wrapBareStringRoot(e){const n=e.schema?.input_schema.required;if(!Array.isArray(n)||1!==n.length)return;const o=n[0];if("string"!=typeof o)return;const r=e.schema?.input_schema.properties[o],s=!!r&&"object"==typeof r&&"array"===r.type;return{[o]:s?[e.text]:e.text}}function coerceToolInput(e){const{raw:n,toolName:o,schema:r}=e,s=Array.isArray(n),a=null===n;let i=n;s&&1===n.length&&(i=n[0]);let l={};if(i&&"object"==typeof i&&!Array.isArray(i))l=i;else if("string"==typeof i&&""!==i.trim()){let e=i,n=!0;try{const o=JSON.parse(i);o&&"object"==typeof o&&!Array.isArray(o)?(l=o,n=!1):"string"==typeof o?e=o:n=!1}catch{}n&&0===Object.keys(l).length&&(l=wrapBareStringRoot({text:e,schema:r})??l)}return s||a?{value:l,coercion:{toolName:o,rawType:s?"array":"null",recovered:Object.keys(l).length>0,...s?{arrayLength:n.length}:{}}}:{value:l}};
module.exports={coerceToolInput,wrapBareStringRoot,resolveToolNameAlias};
