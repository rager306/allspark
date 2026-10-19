
import json, os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
OUT=os.environ.get("MOCK_OUT","/tmp/cx/reqs3")
PLAN=os.environ.get("MOCK_PLAN","/tmp/cx/plan.json")
PORT=int(os.environ.get("MOCK_PORT","8797"))
os.makedirs(OUT, exist_ok=True)
N=[0]
def sse(events):
    return b"".join(f"event: {e}\ndata: {json.dumps(p)}\n\n".encode() for e,p in events)
class H(BaseHTTPRequestHandler):
    protocol_version="HTTP/1.1"
    def log_message(self,*a): pass
    def do_POST(self):
        n=int(self.headers.get('content-length',0)); body=self.rfile.read(n)
        N[0]+=1; idx=N[0]
        d=json.loads(body)
        open(f"{OUT}/req_{idx}.json","w").write(json.dumps(d,indent=1))
        plan=json.load(open(PLAN))
        step = plan[idx-1] if idx-1 < len(plan) else {"items":[{"type":"message","content":[{"type":"output_text","text":"done"}]}]}
        items=step.get("items",[])
        events=[("response.created",{"type":"response.created","response":{"id":"resp_1","object":"response","created_at":0,"status":"in_progress","model":d.get("model","m"),"output":[]}})]
        out=[]
        for oi,it in enumerate(items):
            it=dict(it); it.setdefault("id",f"it_{idx}_{oi}"); it.setdefault("status","completed")
            if it.get("type")=="message": it.setdefault("role","assistant")
            if it.get("type")=="function_call":
                events.append(("response.output_item.added",{"type":"response.output_item.added","output_index":oi,"item":{**it,"status":"in_progress","arguments":""}}))
                events.append(("response.function_call_arguments.delta",{"type":"response.function_call_arguments.delta","item_id":it["id"],"output_index":oi,"delta":it.get("arguments","{}")}))
            elif it.get("type")=="custom_tool_call":
                events.append(("response.output_item.added",{"type":"response.output_item.added","output_index":oi,"item":{**it,"status":"in_progress","input":""}}))
                events.append(("response.custom_tool_call_input.delta",{"type":"response.custom_tool_call_input.delta","item_id":it["id"],"output_index":oi,"delta":it.get("input","")}))
            events.append(("response.output_item.done",{"type":"response.output_item.done","output_index":oi,"item":it}))
            out.append(it)
        resp={"id":"resp_1","object":"response","created_at":0,"status":"completed","model":d.get("model","m"),
              "output":out,"parallel_tool_calls":True,"tool_choice":"auto","tools":[],
              "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2,
                       "input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}
        events.append(("response.completed",{"type":"response.completed","response":resp}))
        buf=sse(events)
        self.send_response(200); self.send_header("content-type","text/event-stream")
        self.send_header("content-length",str(len(buf))); self.end_headers(); self.wfile.write(buf)
ThreadingHTTPServer(("127.0.0.1",PORT),H).serve_forever()
