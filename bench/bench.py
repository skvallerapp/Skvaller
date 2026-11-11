#!/usr/bin/env python3
"""Compare the CPU and memory use of two Skvaller builds: the Tauri one and the all-Rust one.

Each run starts an app on the same fixture (5,000 articles, 3,334 thumbnails, one local
account), then measures the app and every process it runs (WebKit's included):

  launch  10 s after its window appears
  idle    the 30 s after that
  scroll  360 mouse-wheel notches over the article list, 30 ms apart

Linux (X11, or Xwayland: both apps are made to use it) and macOS. Python 3.9+, standard
library only. See README.md next to this file.
"""

import argparse
import ctypes
import datetime
import lzma
import os
import platform
import re
import shutil
import signal
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
APP_ID = "io.github.skvallerapp.Skvaller"
MAC = sys.platform == "darwin"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--tauri", required=True, help="the Tauri build's binary")
    parser.add_argument("--rust", required=True, help="the all-Rust build's binary")
    parser.add_argument("--machine", required=True, help="a short name for the results file, like steam-deck or macbook-m1")
    parser.add_argument("--runs", type=int, default=2, help="measured runs per app (default 2)")
    parser.add_argument("--notches", type=int, default=360)
    parser.add_argument("--wheel-delta", type=int, default=-1,
                        help="macOS only: lines per scroll event; flip the sign if the list scrolls up")
    parser.add_argument("--no-warmup", action="store_true",
                        help="skip the unmeasured first launch of each app that fills shader caches")
    args = parser.parse_args()

    for name in ("skvaller", "Skvaller"):
        if pids_named(name):
            sys.exit(f"A Skvaller process is already running ({name}); quit it first. "
                     "The Tauri build hands off to a running instance and exits.")

    home = os.path.join(HERE, ".home")
    os.makedirs(home, exist_ok=True)
    plat = Mac(home) if MAC else Linux(home)
    apps = [("tauri", os.path.abspath(args.tauri)), ("rust", os.path.abspath(args.rust))]
    for _, binary in apps:
        if not os.access(binary, os.X_OK):
            sys.exit(f"{binary} is not an executable")

    if not args.no_warmup:
        for label, binary in apps:
            print(f"warm-up: {label}", flush=True)
            run(plat, label, binary, args, measure=False)

    results = []
    for i in range(1, args.runs + 1):
        for label, binary in apps:
            print(f"run {i}: {label}", flush=True)
            result = run(plat, f"{label}{i}", binary, args, measure=True)
            print("  " + summary_line(result, plat), flush=True)
            results.append(result)

    path = write_report(args, plat, apps, results)
    print(f"\nResults: {path}")


def run(plat, label, binary, args, measure):
    plat.reset_data()
    before = plat.before_launch()
    log = open(os.path.join(plat.home, f"{label}.log"), "w")
    started = time.time()
    proc = subprocess.Popen([binary], env=plat.env(), stdout=log, stderr=subprocess.STDOUT)
    try:
        window = None
        while window is None:
            if proc.poll() is not None:
                sys.exit(f"{label} exited with {proc.returncode} before showing a window; see {log.name}")
            if time.time() - started > 120:
                sys.exit(f"{label} showed no window in 120 s; see {log.name}")
            time.sleep(0.2)
            window = plat.window(proc.pid)
        window_after = time.time() - started
        time.sleep(10)
        if not measure:
            return None
        pids = plat.processes(proc.pid, before)
        cpu_launch = plat.cpu(pids)
        mem_launch = plat.memory(pids)
        time.sleep(30)
        pids = plat.processes(proc.pid, before)
        cpu_idle = plat.cpu(pids) - cpu_launch
        mem_idle = plat.memory(pids)

        window = plat.window(proc.pid) or window
        x, y = window[0] + 400, window[1] + 400  # over the article list in both apps
        plat.prepare_scroll(proc.pid, x, y)
        time.sleep(2)
        pids = plat.processes(proc.pid, before)
        cpu_before = plat.cpu(pids)
        threads_before = plat.threads(pids)
        plat.scroll(proc.pid, x, y, args.notches, args.wheel_delta)
        time.sleep(3)
        pids = plat.processes(proc.pid, before)
        cpu_scroll = plat.cpu(pids) - cpu_before
        threads = diff_threads(threads_before, plat.threads(pids))
        mem_scroll = plat.memory(pids)
        shot = plat.screenshot(os.path.join(HERE, "results", f"{args.machine}-{label}.png"))
        return {
            "label": label, "window_after": window_after, "procs": len(pids),
            "cpu_launch": cpu_launch, "mem_launch": mem_launch,
            "cpu_idle": cpu_idle, "mem_idle": mem_idle,
            "cpu_scroll": cpu_scroll, "mem_scroll": mem_scroll,
            "per_process": plat.per_process(pids), "threads": threads, "screenshot": shot,
        }
    finally:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        log.close()
        time.sleep(2)


def diff_threads(before, after):
    out = {name: after[name] - before.get(name, 0.0) for name in after}
    return sorted(((s, n) for n, s in out.items() if s >= 0.05), reverse=True)[:12]


def summary_line(r, plat):
    a, b = plat.mem_names
    fmt = lambda m: f"{m[0]:.0f}/{m[1]:.0f} MB"
    return (f"window after {r['window_after']:.1f} s | procs {r['procs']} | "
            f"launch cpu {r['cpu_launch']:.2f} s, {a}/{b} {fmt(r['mem_launch'])} | "
            f"idle 30 s cpu {r['cpu_idle']:.2f} s, {fmt(r['mem_idle'])} | "
            f"scroll cpu {r['cpu_scroll']:.2f} s, {fmt(r['mem_scroll'])}")


def write_report(args, plat, apps, results):
    a, b = plat.mem_names
    os.makedirs(os.path.join(HERE, "results"), exist_ok=True)
    path = os.path.join(HERE, "results", f"{args.machine}.md")
    lines = [
        f"# {args.machine}",
        "",
        f"- Date: {datetime.date.today().isoformat()}",
        f"- Machine: {plat.machine_info()}",
        f"- Builds: " + ", ".join(f"{label} `{git_rev(binary)}`" for label, binary in apps),
        f"- Settings: {args.runs} runs, {args.notches} notches, warm-up {'off' if args.no_warmup else 'on'}",
        f"- Memory columns: {a} / {b} in MB, summed over the app and its child processes. {plat.mem_note}",
        "",
        f"| Run | Window after | Procs | Launch CPU | Launch {a}/{b} | Idle 30 s CPU | Idle {a}/{b} | Scroll CPU | After scroll {a}/{b} |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for r in results:
        m = lambda k: f"{r[k][0]:.0f} / {r[k][1]:.0f}"
        lines.append(f"| {r['label']} | {r['window_after']:.1f} s | {r['procs']} | {r['cpu_launch']:.2f} s | "
                     f"{m('mem_launch')} | {r['cpu_idle']:.2f} s | {m('mem_idle')} | {r['cpu_scroll']:.2f} s | "
                     f"{m('mem_scroll')} |")
    for r in results:
        lines += ["", f"## {r['label']}", "", f"Processes after the scroll ({a} / {b} MB):", ""]
        lines += [f"- {name}: {x:.0f} / {y:.0f}" for name, x, y in r["per_process"]]
        if r["threads"]:
            lines += ["", "Scroll CPU by thread:", ""]
            lines += [f"- {name}: {s:.2f} s" for s, name in r["threads"]]
            if any("llvmpipe" in name or "lavapipe" in name for _, name in r["threads"]):
                lines += ["", "**Warning: llvmpipe threads ran, so this app rendered in software, not on the GPU.**"]
        lines += ["", f"Screenshot after the scroll: `{r['screenshot'] or 'none taken'}`"]
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    return path


def git_rev(binary):
    try:
        return subprocess.run(["git", "-C", os.path.dirname(binary), "log", "-1", "--format=%h %s"],
                              capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def pids_named(name):
    out = subprocess.run(["pgrep", "-x", name], capture_output=True, text=True).stdout
    return [int(p) for p in out.split()]


def output(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=30).stdout
    except (OSError, subprocess.TimeoutExpired):
        return ""


class Platform:
    def __init__(self, home):
        self.home = home

    def reset_data(self):
        """Puts a fresh copy of the fixture in place; caches outside it are kept."""
        for d in (self.data_dir, self.config_dir):
            shutil.rmtree(d, ignore_errors=True)
        os.makedirs(os.path.join(self.data_dir, "accounts"))
        os.makedirs(self.config_dir, exist_ok=True)
        with lzma.open(os.path.join(HERE, "fixture", "1.db.xz")) as src, \
                open(os.path.join(self.data_dir, "accounts", "1.db"), "wb") as dst:
            shutil.copyfileobj(src, dst)
        shutil.copy(os.path.join(HERE, "fixture", "config.json"), os.path.join(self.config_dir, "config.json"))

    def before_launch(self):
        return None

    def threads(self, pids):
        return {}


class Linux(Platform):
    mem_names = ("PSS", "RSS")
    mem_note = "PSS splits shared pages between processes; use it to compare."

    def __init__(self, home):
        super().__init__(home)
        self.data_dir = os.path.join(home, "data", APP_ID)
        self.config_dir = os.path.join(home, "config", APP_ID)
        self.tck = os.sysconf("SC_CLK_TCK")
        self.x = Xlib()

    def env(self):
        env = dict(os.environ)
        env.update(XDG_DATA_HOME=os.path.join(self.home, "data"),
                   XDG_CONFIG_HOME=os.path.join(self.home, "config"),
                   XDG_CACHE_HOME=os.path.join(self.home, "cache"),
                   GDK_BACKEND="x11")  # both apps on X11/Xwayland, where XTest can scroll them
        env.pop("WAYLAND_DISPLAY", None)
        return env

    def window(self, pid):
        return self.x.window_of(pid)

    def processes(self, root, _before):
        out, todo = [], [root]
        while todo:
            p = todo.pop()
            out.append(p)
            try:
                for task in os.listdir(f"/proc/{p}/task"):
                    with open(f"/proc/{p}/task/{task}/children") as f:
                        todo += [int(c) for c in f.read().split()]
            except OSError:
                pass
        return out

    def _stat(self, path):
        with open(path) as f:
            fields = f.read().rsplit(")", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / self.tck  # utime + stime

    def cpu(self, pids):
        total = 0.0
        for p in pids:
            try:
                total += self._stat(f"/proc/{p}/stat")
            except OSError:
                pass
        return total

    def _rollup(self, pid):
        vals = {}
        with open(f"/proc/{pid}/smaps_rollup") as f:
            for line in f:
                k, _, v = line.partition(":")
                if k in ("Rss", "Pss"):
                    vals[k] = int(v.split()[0]) / 1024
        return vals.get("Pss", 0.0), vals.get("Rss", 0.0)

    def memory(self, pids):
        pss = rss = 0.0
        for p in pids:
            try:
                a, b = self._rollup(p)
                pss, rss = pss + a, rss + b
            except OSError:
                pass
        return pss, rss

    def per_process(self, pids):
        out = []
        for p in pids:
            try:
                with open(f"/proc/{p}/comm") as f:
                    name = f.read().strip()
                out.append((name, *self._rollup(p)))
            except OSError:
                pass
        return out

    def threads(self, pids):
        """CPU seconds per process:thread name, digits stripped (llvmpipe-0..N add up)."""
        out = {}
        for p in pids:
            try:
                with open(f"/proc/{p}/comm") as f:
                    proc = f.read().strip()
                for t in os.listdir(f"/proc/{p}/task"):
                    with open(f"/proc/{p}/task/{t}/comm") as f:
                        name = f"{proc}: " + re.sub(r"[\d:]+$", "", f.read().strip())
                    out[name] = out.get(name, 0.0) + self._stat(f"/proc/{p}/task/{t}/stat")
            except OSError:
                pass
        return out

    def prepare_scroll(self, pid, x, y):
        self.x.move(x, y)

    def scroll(self, pid, x, y, notches, _delta):
        for _ in range(notches):
            self.x.wheel_down()
            time.sleep(0.03)

    def screenshot(self, path):
        for cmd in (["spectacle", "-b", "-n", "-f", "-o", path],
                    ["distrobox-host-exec", "spectacle", "-b", "-n", "-f", "-o", path],
                    ["import", "-window", "root", path],
                    ["grim", path]):
            if shutil.which(cmd[0]) and subprocess.run(cmd, capture_output=True, timeout=30).returncode == 0 \
                    and os.path.exists(path):
                return os.path.relpath(path, HERE)
        return None

    def machine_info(self):
        cpu = re.search(r"model name\s*:\s*(.+)", open("/proc/cpuinfo").read())
        mem = re.search(r"MemTotal:\s*(\d+)", open("/proc/meminfo").read())
        os_name = re.search(r'PRETTY_NAME="(.+)"', open("/etc/os-release").read())
        session = os.environ.get("XDG_SESSION_TYPE", "?")
        return (f"{cpu.group(1) if cpu else '?'}, {int(mem.group(1)) // 1024} MB RAM, "
                f"{os_name.group(1) if os_name else '?'} (kernel {platform.release()}), session {session}, "
                f"apps on X11/Xwayland")


class XWindowAttributes(ctypes.Structure):
    _fields_ = [("x", ctypes.c_int), ("y", ctypes.c_int), ("width", ctypes.c_int), ("height", ctypes.c_int),
                ("border_width", ctypes.c_int), ("depth", ctypes.c_int), ("visual", ctypes.c_void_p),
                ("root", ctypes.c_ulong), ("class", ctypes.c_int), ("bit_gravity", ctypes.c_int),
                ("win_gravity", ctypes.c_int), ("backing_store", ctypes.c_int),
                ("backing_planes", ctypes.c_ulong), ("backing_pixel", ctypes.c_ulong),
                ("save_under", ctypes.c_int), ("colormap", ctypes.c_ulong), ("map_installed", ctypes.c_int),
                ("map_state", ctypes.c_int), ("all_event_masks", ctypes.c_long),
                ("your_event_mask", ctypes.c_long), ("do_not_propagate_mask", ctypes.c_long),
                ("override_redirect", ctypes.c_int), ("screen", ctypes.c_void_p)]


class Xlib:
    """Just enough Xlib + XTest, through ctypes, to find a window by PID and scroll it."""

    def __init__(self):
        try:
            self.x = ctypes.cdll.LoadLibrary("libX11.so.6")
            self.t = ctypes.cdll.LoadLibrary("libXtst.so.6")
        except OSError as e:
            sys.exit(f"{e}: install libX11 and libXtst (Arch: libxtst, Debian: libxtst6)")
        x, c = self.x, ctypes
        x.XOpenDisplay.restype = c.c_void_p
        x.XOpenDisplay.argtypes = [c.c_char_p]
        x.XDefaultRootWindow.restype = c.c_ulong
        x.XDefaultRootWindow.argtypes = [c.c_void_p]
        x.XInternAtom.restype = c.c_ulong
        x.XInternAtom.argtypes = [c.c_void_p, c.c_char_p, c.c_int]
        x.XQueryTree.argtypes = [c.c_void_p, c.c_ulong, c.POINTER(c.c_ulong), c.POINTER(c.c_ulong),
                                 c.POINTER(c.POINTER(c.c_ulong)), c.POINTER(c.c_uint)]
        x.XGetWindowProperty.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_long, c.c_long, c.c_int,
                                         c.c_ulong, c.POINTER(c.c_ulong), c.POINTER(c.c_int),
                                         c.POINTER(c.c_ulong), c.POINTER(c.c_ulong),
                                         c.POINTER(c.POINTER(c.c_ubyte))]
        x.XGetWindowAttributes.argtypes = [c.c_void_p, c.c_ulong, c.POINTER(XWindowAttributes)]
        x.XTranslateCoordinates.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_int, c.c_int,
                                            c.POINTER(c.c_int), c.POINTER(c.c_int), c.POINTER(c.c_ulong)]
        x.XFree.argtypes = [c.c_void_p]
        x.XFlush.argtypes = [c.c_void_p]
        x.XSync.argtypes = [c.c_void_p, c.c_int]
        self.t.XTestFakeMotionEvent.argtypes = [c.c_void_p, c.c_int, c.c_int, c.c_int, c.c_ulong]
        self.t.XTestFakeButtonEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
        # Windows can vanish mid-walk; the default handler would exit the process.
        self._handler = c.CFUNCTYPE(c.c_int, c.c_void_p, c.c_void_p)(lambda d, e: 0)
        x.XSetErrorHandler(self._handler)
        self.d = x.XOpenDisplay(None)
        if not self.d:
            sys.exit("Can't open the X display: is DISPLAY set? (Xwayland provides one on Wayland)")
        self.root = x.XDefaultRootWindow(self.d)
        self.pid_atom = x.XInternAtom(self.d, b"_NET_WM_PID", 0)

    def _pid(self, w):
        c = ctypes
        typ, fmt, n, after = c.c_ulong(), c.c_int(), c.c_ulong(), c.c_ulong()
        data = c.POINTER(c.c_ubyte)()
        if self.x.XGetWindowProperty(self.d, w, self.pid_atom, 0, 1, 0, 6, c.byref(typ), c.byref(fmt),
                                     c.byref(n), c.byref(after), c.byref(data)) != 0 or not data:
            return None
        pid = c.cast(data, c.POINTER(c.c_ulong))[0] if n.value else None
        self.x.XFree(data)
        return pid

    def _children(self, w):
        c = ctypes
        r, p, kids, n = c.c_ulong(), c.c_ulong(), c.POINTER(c.c_ulong)(), c.c_uint()
        if not self.x.XQueryTree(self.d, w, c.byref(r), c.byref(p), c.byref(kids), c.byref(n)):
            return []
        out = [kids[i] for i in range(n.value)]
        if kids:
            self.x.XFree(kids)
        return out

    def window_of(self, pid):
        """(x, y, width, height) on the root of pid's largest viewable window, or None."""
        best, todo = None, [self.root]
        while todo:
            w = todo.pop()
            todo += self._children(w)
            if self._pid(w) != pid:
                continue
            a = XWindowAttributes()
            if not self.x.XGetWindowAttributes(self.d, w, ctypes.byref(a)) or a.map_state != 2:  # IsViewable
                continue
            if a.width < 300 or a.height < 300:
                continue
            rx, ry, child = ctypes.c_int(), ctypes.c_int(), ctypes.c_ulong()
            self.x.XTranslateCoordinates(self.d, w, self.root, 0, 0, ctypes.byref(rx), ctypes.byref(ry),
                                         ctypes.byref(child))
            if best is None or a.width * a.height > best[2] * best[3]:
                best = (rx.value, ry.value, a.width, a.height)
        return best

    def move(self, x, y):
        self.t.XTestFakeMotionEvent(self.d, -1, x, y, 0)
        self.x.XSync(self.d, 0)

    def wheel_down(self):
        self.t.XTestFakeButtonEvent(self.d, 5, 1, 0)
        self.t.XTestFakeButtonEvent(self.d, 5, 0, 0)
        self.x.XFlush(self.d)


class Mac(Platform):
    mem_names = ("footprint", "RSS")
    mem_note = ("Footprint is what Activity Monitor shows as Memory; use it to compare. "
                "WebKit's processes are found as the com.apple.WebKit.* processes that appear after launch.")

    def __init__(self, home):
        super().__init__(home)
        support = os.path.join(home, "Library", "Application Support", APP_ID)
        self.data_dir = self.config_dir = support
        self.helper = os.path.join(home, "mac-input")
        source = os.path.join(HERE, "mac-input.swift")
        if not os.path.exists(self.helper) or os.path.getmtime(self.helper) < os.path.getmtime(source):
            subprocess.run(["swiftc", "-O", "-o", self.helper, source], check=True)
        check = subprocess.run([self.helper, "trusted"])
        if check.returncode != 0:
            sys.exit("Posting scroll events needs Accessibility access for the app running this script "
                     "(System Settings > Privacy & Security > Accessibility), then run it again.")

    def env(self):
        env = dict(os.environ)
        env["HOME"] = self.home  # both apps find Application Support under HOME
        return env

    def before_launch(self):
        return set(self._webkit())

    def _webkit(self):
        return [int(p) for p in output(["pgrep", "-f", "com.apple.WebKit"]).split()]

    def window(self, pid):
        out = output([self.helper, "window", str(pid)]).split()
        return tuple(int(float(v)) for v in out) if len(out) == 4 else None

    def processes(self, root, before):
        return [root] + [p for p in self._webkit() if p not in before]

    def _ps(self, pids, field):
        return output(["ps", "-o", f"pid=,{field}=", "-p", ",".join(map(str, pids))]).splitlines()

    def cpu(self, pids):
        total = 0.0
        for line in self._ps(pids, "time"):
            parts = line.split()
            if len(parts) == 2:
                secs = 0.0
                for part in parts[1].split(":"):
                    secs = secs * 60 + float(part)
                total += secs
        return total

    def _footprints(self, pids):
        """MB per PID from top's MEM column (the physical footprint)."""
        out = {}
        for p in pids:
            for line in output(["top", "-l", "1", "-stats", "pid,mem", "-pid", str(p)]).splitlines():
                m = re.match(r"\s*(\d+)\s+([\d.]+)([BKMG])", line)
                if m and int(m.group(1)) == p:
                    scale = {"B": 1 / 1048576, "K": 1 / 1024, "M": 1, "G": 1024}[m.group(3)]
                    out[p] = float(m.group(2)) * scale
        return out

    def _rss(self, pids):
        out = {}
        for line in self._ps(pids, "rss"):
            parts = line.split()
            if len(parts) == 2:
                out[int(parts[0])] = int(parts[1]) / 1024
        return out

    def memory(self, pids):
        return sum(self._footprints(pids).values()), sum(self._rss(pids).values())

    def per_process(self, pids):
        fp, rss = self._footprints(pids), self._rss(pids)
        names = {}
        for line in self._ps(pids, "comm"):
            pid, _, comm = line.strip().partition(" ")
            names[int(pid)] = os.path.basename(comm.strip())
        return [(names.get(p, str(p)), fp.get(p, 0.0), rss.get(p, 0.0)) for p in pids if p in names]

    def prepare_scroll(self, pid, x, y):
        subprocess.run([self.helper, "focus", str(pid), str(x), str(y)], check=True)

    def scroll(self, pid, x, y, notches, delta):
        subprocess.run([self.helper, "scroll", str(notches), str(delta), "30"], check=True)

    def screenshot(self, path):
        if subprocess.run(["screencapture", "-x", path], capture_output=True).returncode == 0 \
                and os.path.exists(path):
            return os.path.relpath(path, HERE)
        return None

    def machine_info(self):
        chip = output(["sysctl", "-n", "machdep.cpu.brand_string"]).strip()
        mem = int(output(["sysctl", "-n", "hw.memsize"]).strip() or 0) // 1048576
        return f"{chip}, {mem} MB RAM, macOS {platform.mac_ver()[0]}"


if __name__ == "__main__":
    main()
