
// Используем fetch() как 9router
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

fetch('https://opencode.ai/zen/v1/chat/completions', {
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    'Authorization': 'Bearer public',
    'User-Agent': 'opencode/1.18.31',
    'x-opencode-client': 'desktop',
    'x-opencode-session': 'ses_a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4',
    'x-opencode-request': 'msg_a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4',
    'x-opencode-project': 'global',
  },
  body
}).then(r => r.json()).then(d => {
  if (d.choices) {
    console.log('SUCCESS:', d.choices[0].message.content);
  } else {
    console.log('ERROR:', JSON.stringify(d).slice(0,200));
  }
}).catch(e => console.log('FETCH_ERR:', e.message));
