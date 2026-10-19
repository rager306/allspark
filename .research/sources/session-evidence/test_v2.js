
const crypto = require('crypto');

function unstableRandom() {
  const bytes = crypto.randomBytes(14);
  const chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  let s = "";
  for (let i = 0; i < 14; i++) s += chars[bytes[i] % 62];
  return s;
}

function descendingTimeHex(time, counterValue) {
  const MASK = (1n << 48n) - 1n;
  return (~(BigInt(time) * 0x1000n + BigInt(counterValue)) & MASK)
    .toString(16).padStart(12, "0");
}

const ts = Date.now();
const session = "ses_" + descendingTimeHex(ts, 1) + unstableRandom();

const body = JSON.stringify({
  model: "deepseek-v4.1-flash-free",
  messages: [{role: "user", content: "What is 2+2? Just the number."}],
  max_tokens: 20,
  stream: false,
  tools: [
    {type:"function",function:{name:"bash",description:"x",parameters:{type:"object",properties:{}}}},
    {type:"function",function:{name:"glob",description:"x",parameters:{type:"object",properties:{}}}},
    {type:"function",function:{name:"grep",description:"x",parameters:{type:"object",properties:{}}}},
    {type:"function",function:{name:"read",description:"x",parameters:{type:"object",properties:{}}}}
  ]
});

fetch('https://opencode.ai/inference/v1/chat/completions', {
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    'Authorization': 'Bearer public',
    'User-Agent': 'opencode/latest/2.0.26/cli',
    'x-opencode-client': 'cli',
    'x-opencode-session': session,
    'x-opencode-session-id': session,
    'x-session-affinity': session,
    'x-session-id': session,
    'x-opencode-project': 'global',
    'Accept': '*/*',
  },
  body
}).then(r => r.json()).then(d => {
  if (d.choices) console.log('SUCCESS:', d.choices[0].message.content);
  else console.log('ERROR:', JSON.stringify(d).slice(0,200));
}).catch(e => console.log('ERR:', e.message));
