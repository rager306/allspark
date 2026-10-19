
const https = require('https');
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
const req = https.request({
  hostname: 'opencode.ai',
  path: '/zen/v1/chat/completions',
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    'Authorization': 'Bearer public',
    'User-Agent': 'opencode/1.18.31',
    'x-opencode-client': 'desktop',
    'x-opencode-session': 'ses_a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4',
    'x-opencode-request': 'msg_a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4',
    'x-opencode-project': 'global',
  }
}, (res) => {
  let data = '';
  res.on('data', c => data += c);
  res.on('end', () => {
    try {
      const j = JSON.parse(data);
      const content = j.choices?.[0]?.message?.content || JSON.stringify(j).slice(0,200);
      console.log('RESULT:', content);
    } catch(e) { console.log('RAW:', data.slice(0,300)); }
  });
});
req.write(body);
req.end();
