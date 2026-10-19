
const __name=(fn,name)=>{try{Object.defineProperty(fn,'name',{value:name})}catch{};return fn};
function readString2({input,key}){const v=input?.[key];return typeof v==='string'?v:undefined}
function errorResult({error}){return{ok:false,error}}
function textResult({text}){return{ok:true,content:[{type:'text',text}]}}

const gk=/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\.|$)/;
function parseCategories(e){const n=[];for(const o of e.split(/^# /gm).slice(1)){if(!o.trim())continue;const e=o.split("\n"),r=(e[0]??"").trim();if(o.includes("See ["))continue;const s=e.filter(e=>e.trim().startsWith("- ")&&e.includes("Confidence:"));0!==s.length&&n.push({name:r,learningCount:s.length,learnings:s,fullSection:`# ${o}`})}return n}function categorySlug(e){const n=e.toLowerCase().replace(/[\s<>:"/\\|?*]+/g,"-").replace(/^[-.]+|[-.]+$/g,"");return gk.test(n)?`${n}-`:n}async function reorganizeIfNeeded(e){const{runtime:n,dir:o,emit:r}=e,s=`${o}/taste.md`;if(!await n.fs.exists({path:s}))return;const a=await n.fs.read({path:s}),i=parseCategories(a).filter(e=>e.learningCount>5);if(0===i.length)return;let l=a;for(const e of i){const s=categorySlug(e.name);if(!s)continue;const a=`${o}/${s}`;await n.fs.mkdir({path:a});const i=`# ${e.name}\n${e.learnings.join("\n")}\n`;await n.fs.write({path:`${a}/taste.md`,content:i});const c=`# ${e.name}\nSee [${s}/taste.md](${s}/taste.md)\n`;l=l.replace(e.fullSection,c),r?.({type:"notice",level:"info",message:`taste: moved ${e.name} → ${s}/taste.md`})}await n.fs.write({path:s,content:l})};
module.exports={parseCategories,categorySlug,reorganizeIfNeeded};
