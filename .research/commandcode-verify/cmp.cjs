
const __name=(fn,name)=>{try{Object.defineProperty(fn,'name',{value:name})}catch{};return fn};
function readString2({input,key}){const v=input?.[key];return typeof v==='string'?v:undefined}
function errorResult({error}){return{ok:false,error}}
function textResult({text}){return{ok:true,content:[{type:'text',text}]}}

function extractLearningWithConfidence(e){const n=e.line.match(/^\s*-\s+(.+?)\.\s+Confidence:\s*(\d*\.?\d+)/),o=n?.[1],r=n?.[2];if(void 0===o||void 0===r)return null;const s=Number.parseFloat(r);return Number.isNaN(s)?null:{text:o.trim(),confidence:s}}function extractLearnings(e){const n=[];for(const o of e.text.split("\n")){if(!o.trim().startsWith("-"))continue;const e=extractLearningWithConfidence({line:o});e&&n.push(e)}return n}function compareLearnings(e){const n=extractLearnings({text:e.oldText}),o=extractLearnings({text:e.newText}),r=new Map(n.map(e=>[e.text,e.confidence])),s=[],a=[],i=[];for(const e of o){const n=r.get(e.text);void 0!==n?e.confidence>n?a.push({text:e.text,oldConfidence:n,newConfidence:e.confidence}):e.confidence<n&&i.push({text:e.text,oldConfidence:n,newConfidence:e.confidence}):s.push(e)}return{added:s,upgraded:a,downgraded:i}};var pk=/^- .*Confidence:.*$/gm;
module.exports={extractLearningWithConfidence,extractLearnings,compareLearnings};
