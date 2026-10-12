"""Mock Xtream portal for Bongo Player end-to-end tests.

    py -3 mock_portal.py PORT SEGDIR LOGFILE [MODE]

player_api.php   login check (user "tester", password "secret"), live / VOD /
                 series lists, EPG
/live/u/p/N.m3u8 302 to /edge/tokN/1: a fresh token per request, like the real
                 portal's edge redirect. The edge serves a sliding live
                 playlist of 4 s segments.
/timeshift/...   302 to /tsplay/vod.m3u8 (catch-up; generating it takes 2.5 s)
/seg/TOK/N.ts    the test segments made by make-segments.ps1

MODE: none (default) | once | always - the first (once) or every (always)
edge token answers 509 from its first reload on.

Environment:
  MOCK_CHANNELS=N    live channels "TEST 01" .. "TEST NN" (default 40)
  MOCK_VARIANTS=1    channel 05 also as "TEST 05 UHD" (105) and "TEST 05 FHD" (205)
  MOCK_ADULT=1       after TEST 02: "24/7 | ADULT A" (901, is_adult=1) and
                     "XXX | ADULT B" (902, is_adult=1; the XXX in its name means
                     nothing, the flag does); VOD "Adult Film" (9001, is_adult=1)
                     and "Gewone Film" (9002)
  MOCK_XXX=1         names that start with XXX but carry no flag, so they are
                     ordinary content: the live channel "XXX Something" (903, last)
                     and the VOD film "xXx: State of the Union" (9003)
  MOCK_NEWS=1        "NL | NPO 1" (501) with archive twin "NL | NPO1 HD" (601,
                     tv_archive=1); the twin's EPG holds three past programmes:
                     NOS Journaal, NOS Jeugdjournaal, Tussen Kunst en Kitsch
  MOCK_DENY=105,...  streams refused to the account (HTTP 407)
  MOCK_BUSY_UNTIL=S  live requests answer 503 for the first S seconds
  MOCK_LATENCY=1     answer about as slowly as the real portal
  MOCK_LOGIN_STATUS=N  the login (player_api.php without an action) answers HTTP N,
                     e.g. 503 or 509, the real portal's busy answers
  MOCK_EPG_DELAY=S   day-EPG requests (the news fetch) answer after S seconds, so
                     a test can press n again while a fetch is in flight
"""
import base64
import http.server
import json
import os
import sys
import threading
import time
import urllib.parse

PORT, SEGDIR, LOG = int(sys.argv[1]), sys.argv[2], sys.argv[3]
MODE = sys.argv[4] if len(sys.argv) > 4 else "none"
ENV = os.environ.get
LATENCY = ENV("MOCK_LATENCY") == "1"
SEG_DUR = 4
NSEG = len([f for f in os.listdir(SEGDIR) if f.endswith(".ts")])
T0 = time.time()
lock = threading.Lock()
edge_hits, dead = {}, set()
state = {"tokens": 0, "failed_once": False}


def log(code, path):
    with lock, open(LOG, "a") as f:
        f.write(f"{time.time() - T0:7.2f}s  {code}  {path}\n")


def b64(s):
    return base64.b64encode(s.encode()).decode()


def live_streams():
    n = int(ENV("MOCK_CHANNELS", "40"))

    def ch(sid, name, archive=0, adult=0):
        return {"stream_id": sid, "name": name, "category_id": "1", "epg_channel_id": "",
                "tv_archive": archive, "is_adult": str(adult)}

    out = []
    for i in range(1, n + 1):
        out.append(ch(i, f"TEST {i:02d}"))
        if i == 2 and ENV("MOCK_ADULT") == "1":
            out += [ch(901, "24/7 | ADULT A", adult=1), ch(902, "XXX | ADULT B", adult=1)]
    if ENV("MOCK_VARIANTS") == "1":
        out += [ch(105, "TEST 05 UHD"), ch(205, "TEST 05 FHD")]
    if ENV("MOCK_NEWS") == "1":
        out += [ch(501, "NL | NPO 1"), ch(601, "NL | NPO1 HD", archive=1)]
    if ENV("MOCK_XXX") == "1":
        out += [ch(903, "XXX Something")]
    return out


def vod_streams():
    films = []
    if ENV("MOCK_ADULT") == "1":
        films += [{"stream_id": 9001, "name": "Adult Film", "container_extension": "mp4", "is_adult": "1"},
                  {"stream_id": 9002, "name": "Gewone Film", "container_extension": "mp4", "is_adult": "0"}]
    if ENV("MOCK_XXX") == "1":
        films += [{"stream_id": 9003, "name": "xXx: State of the Union", "container_extension": "mp4",
                   "is_adult": "0"}]
    return films


def news_epg():
    now = int(time.time())
    progs = [("NOS Journaal", now - 7200, now - 6600),
             ("NOS Jeugdjournaal", now - 3600, now - 3000),
             ("Tussen Kunst en Kitsch", now - 2400, now + 1200)]
    fmt = lambda t: time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(t))
    return [{"id": str(i), "epg_id": str(i), "title": b64(t), "lang": "", "start": fmt(s), "end": fmt(e),
             "description": b64(""), "channel_id": "npo1", "start_timestamp": str(s),
             "stop_timestamp": str(e), "now_playing": 0, "has_archive": 1}
            for i, (t, s, e) in enumerate(progs, 1)]


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def reply(self, code, body=b"", ctype="text/plain", headers=None):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)
        log(code, self.path)

    def json(self, obj):
        return self.reply(200, json.dumps(obj).encode(), "application/json")

    def do_GET(self):
        p = self.path
        if LATENCY:
            time.sleep(0.30 if p.startswith("/live/") else 0.12 if p.startswith("/edge/") else 0.05)
        if p.startswith("/player_api.php"):
            q = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(p).query))
            action = q.get("action")
            if action is None:
                if ENV("MOCK_LOGIN_STATUS"):
                    return self.reply(int(ENV("MOCK_LOGIN_STATUS")), b"busy")
                ok = q.get("username") == "tester" and q.get("password") == "secret"
                return self.json({"user_info": {"auth": 1 if ok else 0, "status": "Active"}, "server_info": {}})
            if action == "get_live_streams":
                return self.json(live_streams())
            if action == "get_vod_streams":
                return self.json(vod_streams())
            if action == "get_simple_data_table" and ENV("MOCK_EPG_DELAY"):
                time.sleep(float(ENV("MOCK_EPG_DELAY")))
            if action in ("get_simple_data_table", "get_short_epg"):
                if q.get("stream_id") == "601" and ENV("MOCK_NEWS") == "1":
                    return self.json({"epg_listings": news_epg()})
                return self.json({"epg_listings": []})
            return self.json([])
        if p.startswith("/timeshift/"):
            return self.reply(302, headers={"Location": f"http://127.0.0.1:{PORT}/tsplay/vod.m3u8"})
        if p.startswith("/tsplay/"):
            time.sleep(2.5)
            lines = ["#EXTM3U", "#EXT-X-VERSION:3", f"#EXT-X-TARGETDURATION:{SEG_DUR}",
                     "#EXT-X-MEDIA-SEQUENCE:0", "#EXT-X-PLAYLIST-TYPE:VOD"]
            for s in range(NSEG):
                lines += [f"#EXTINF:{SEG_DUR}.000,", f"/seg/vod/{s}.ts"]
            lines.append("#EXT-X-ENDLIST")
            return self.reply(200, ("\n".join(lines) + "\n").encode(), "application/vnd.apple.mpegurl")
        if p.startswith("/live/") and p.endswith(".m3u8"):
            sid = p.rsplit("/", 1)[1].split(".")[0]
            if sid in ENV("MOCK_DENY", "").split(","):
                return self.reply(407)
            if time.time() - T0 < float(ENV("MOCK_BUSY_UNTIL", "0")):
                return self.reply(503, b"Service Unavailable")
            with lock:
                state["tokens"] += 1
                tok = state["tokens"]
            return self.reply(302, headers={"Location": f"http://127.0.0.1:{PORT}/edge/tok{tok}/1"})
        if p.startswith("/edge/"):
            tok = p.split("/")[2]
            with lock:
                edge_hits[tok] = edge_hits.get(tok, 0) + 1
                if edge_hits[tok] == 2 and (MODE == "always" or (MODE == "once" and not state["failed_once"])):
                    dead.add(tok)
                    state["failed_once"] = True
                is_dead = tok in dead
            if is_dead:
                return self.reply(509, b"Bandwidth Limit Exceeded")
            first = max(0, int((time.time() - T0) / SEG_DUR) - 2)
            lines = ["#EXTM3U", "#EXT-X-VERSION:3", f"#EXT-X-TARGETDURATION:{SEG_DUR}",
                     f"#EXT-X-MEDIA-SEQUENCE:{first}"]
            for s in range(first, first + 3):
                lines += [f"#EXTINF:{SEG_DUR}.000,", f"/seg/{tok}/{s}.ts"]
            return self.reply(200, ("\n".join(lines) + "\n").encode(), "application/vnd.apple.mpegurl")
        if p.startswith("/seg/"):
            n = int(p.rsplit("/", 1)[1].split(".")[0])
            with open(os.path.join(SEGDIR, f"seg{n % NSEG}.ts"), "rb") as f:
                return self.reply(200, f.read(), "video/mp2t")
        self.reply(404, b"not found")

    def log_message(self, *args):
        pass


http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
