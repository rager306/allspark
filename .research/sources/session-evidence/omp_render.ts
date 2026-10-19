
import { render } from "/tmp/omp-src/packages/utils/src/prompt.ts";
import fs from "node:fs";
const P="/tmp/omp-src/packages/coding-agent/src/prompts/";
const tpl = fs.readFileSync(P+"system/system-prompt.md","utf8");
const proj = fs.readFileSync(P+"system/project-prompt.md","utf8");
const ctx = {
  skills: [], alwaysApplyRules: [], rules: [], internalUrls: [],
  toolInfo: [{name:"read",label:"Read"},{name:"edit",label:"Edit"},{name:"bash",label:"Bash"},{name:"grep",label:"Grep"},{name:"glob",label:"Glob"},{name:"task",label:"Task"},{name:"todo_write",label:"TodoWrite"},{name:"write",label:"Write"},{name:"find",label:"Find"},{name:"lsp",label:"LSP"},{name:"web_search",label:"WebSearch"},{name:"think",label:"Think"},{name:"wait",label:"Wait"}],
  toolListMode: true, toolRefs: {read:"read",edit:"edit",bash:"bash",grep:"grep",glob:"glob",task:"task",todo_write:"todo_write",write:"write",find:"find",lsp:"lsp",web_search:"web_search",think:"think",wait:"wait"},
  tools: ["read","edit","bash","grep","glob","task","todo_write","write","find","lsp","web_search","think","wait"],
  environment: [{label:"OS",value:"linux 6.8.0"},{label:"Arch",value:"x64"}],
  cwd: "/root/prime-agent", additionalWorkspaceRoots: [], model: "anthropic/claude-opus-4",
  delegationBias:"restrained", inlineFirstDelegation:true, eagerTasks:false, MAX_CONCURRENCY:8,
  personality:"", intentTracing:false, secretsEnabled:false, includeWorkspaceTree:false,
  renderMermaid:true, renderSvg:true, autoGraph:true, reactions:false, xdevTools:[], hasDynamicXdevTools:false,
  autoQaEnabled:false, writeTransportOnly:false, subagent:false,
  contextFiles: [], agentsMdSearch: {files: []}, workspaceTree: {rendered:"", truncated:false},
  activeRepoContext: "", appendPrompt: "",
};
const a = render(tpl, ctx);
const b = render(proj, {...ctx, activeRepoContext: ""});
fs.writeFileSync("/tmp/roadmap_audit/_omp_block0.txt", a);
fs.writeFileSync("/tmp/roadmap_audit/_omp_block1.txt", b);
console.log(a.length, b.length);
