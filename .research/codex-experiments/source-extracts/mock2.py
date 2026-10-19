
import json, os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
OUT="/tmp/cx/reqs2"
os.makedirs(OUT, exist_ok=True)
N=[0]
class H(BaseHTTPRequestHandler):
    protocol_version="HTTP/1.1"
    def log_message(self,*a): pass
    def do_POST(self):
        n=int(self.headers.get('content-length',0))
        body=self.rfile.read(n)
        N[0]+=1
        idx=N[0]
        try:
            d=json.loads(body)
            open(f"{OUT}/req_{idx}.json","w").write(json.dumps(d,indent=1))
        except Exception:
            open(f"{OUT}/raw_{idx}.txt","wb").write(body); d={}
        resp={"id":"resp_1","object":"response","created_at":0,"status":"completed",
              "model":d.get("model","m"),
              "output":[{"type":"message","id":"msg_1","status":"completed","role":"assistant",
                         "content":[{"type":"output_text","text":"ok","annotations":[]}]}],
              "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2,
                       "input_tokens_details":{"cached_tokens":0},
                       "output_tokens_details":{"reasoning_tokens":0}}}
        events=[("response.created",{"type":"response.created","response":{**resp,"status":"in_progress","output":[]}}),
                ("response.output_item.added",{"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","status":"in_progress","role":"assistant","content":[]}}),
                ("response.output_text.delta",{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"ok"}),
                ("response.output_item.done",{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_1","status":"completed","role":"assistant","content":[{"type":"output_text","text":"ok","annotations":[]}]}}),
                ("response.completed",{"type":"response.completed","response":resp})]
        buf=b"".join(f"event: {e}\ndata: {json.dumps(p)}\n\n".encode() for e,p in events)
        self.send_response(200)
        self.send_header("content-type","text/event-stream")
        self.send_header("cache-control","no-cache")
        self.send_header("content-length",str(len(buf)))
        self.end_headers()
        self.wfile.write(buf)
ThreadingHTTPServer(("127.0.0.1",8798),H).serve_forever()
