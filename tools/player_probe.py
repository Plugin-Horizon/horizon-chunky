#!/usr/bin/env python3
"""Needs suggest_probe.py (Horizon test client Conn class) next to it. Joins as a player and runs /chunky commands. Usage: player_probe.py <port> <name> <cmd> [<cmd>...]"""
import importlib.util, re, struct, sys, time
spec = importlib.util.spec_from_file_location("sp", __import__("os").path.join(__import__("os").path.dirname(__file__), "suggest_probe.py"))
sp = importlib.util.module_from_spec(spec); spec.loader.exec_module(sp)
port, name, cmds = int(sys.argv[1]), sys.argv[2], sys.argv[3:]
c = sp.Conn(port, name)
got, pos, last = [], None, 0.0
def pump(seconds):
    global pos, last
    end = time.time() + seconds
    while time.time() < end:
        try: pid, d = c.recv()
        except Exception: pid = None
        if pid == 45: c.send(28, d)
        elif pid == 73:
            tid, i = sp.parse_varint(d); pos = struct.unpack(">ddd", d[i:i + 24])
            c.send(0, sp.varint(tid) + struct.pack(">dddff", *pos, 0.0, 0.0)); c.send(44)
        elif pid == 124: got.append(re.sub(rb"[^\x20-\x7e]+", b" ", d).decode().strip())
        if pos and time.time() - last > 0.5 and pid is not None:
            c.send(31, struct.pack(">dddffB", *pos, 0.0, 0.0, 1)); c.send(13); last = time.time()
pump(4)
for cmd in cmds:
    got.clear(); c.send(7, sp.string(cmd)); pump(2.5); print(f"/{cmd} -> {[g for g in got if 'joined' not in g]}")
