"""Run a Java GhidraScript file through ghidra-mcp /run_script_inline (server started with -AllowScripts).
Usage: run_ghidra_java.py Script.java <program> [args...]"""
import json, sys, urllib.request

code = open(sys.argv[1]).read()
body = {'code': code, 'program': sys.argv[2], 'args': ' '.join(sys.argv[3:])}
req = urllib.request.Request('http://127.0.0.1:8089/run_script_inline', json.dumps(body).encode(),
                             {'Content-Type': 'application/json'})
r = json.load(urllib.request.urlopen(req, timeout=7200))
print(json.dumps(r, indent=1)[:4000])
