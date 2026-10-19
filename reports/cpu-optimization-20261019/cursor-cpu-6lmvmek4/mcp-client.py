import json, socket, struct, uuid
from pathlib import Path

class Client:
    def __init__(self, descriptor, expected_pid):
        d=json.loads(Path(descriptor).read_text())
        if d.get('pid') != expected_pid: raise RuntimeError('Descriptor PID mismatch')
        self.socket=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
        self.socket.settimeout(30)
        self.socket.connect(d['socket'])
        self.session='';self.revision=None
        self.request('handshake',{'client_protocol':'snow-shot-mcp/1','token':d['token']})
    def receive(self,n):
        data=bytearray()
        while len(data)<n:
            part=self.socket.recv(n-len(data))
            if not part: raise RuntimeError('MCP connection closed')
            data.extend(part)
        return bytes(data)
    def request(self,method,params=None):
        rid=uuid.uuid4().hex
        request={'protocol':'snow-shot-mcp/1','request_id':rid,'idempotency_key':rid,'method':method,'params':params or {}}
        if self.session:request.update(session_id=self.session,expected_revision=self.revision)
        encoded=json.dumps(request).encode()
        self.socket.sendall(struct.pack('>II',len(encoded)+4,len(encoded))+encoded)
        while True:
            size=struct.unpack('>I',self.receive(4))[0]
            if not 4<=size<=64*1024*1024:raise RuntimeError('Invalid MCP frame length')
            body=self.receive(size);jsize=struct.unpack('>I',body[:4])[0]
            result=json.loads(body[4:4+jsize])
            if result.get('request_id') != rid:continue
            if not result.get('ok'):raise RuntimeError(str(result.get('error',{}).get('code','MCP request failed')))
            if result.get('session_id'):self.session=result['session_id']
            if result.get('revision') is not None:self.revision=result['revision']
            return result
    def close(self):
        try:self.request('snow_shot_screenshot_cancel')
        finally:self.socket.close()
