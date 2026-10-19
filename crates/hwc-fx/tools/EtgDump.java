import ghidra.app.script.GhidraScript;
import ghidra.app.emulator.EmulatorHelper;
import ghidra.program.model.address.Address;
import ghidra.util.task.ConsoleTaskMonitor;
import java.io.*;
import java.nio.file.*;
import java.util.*;

// Runs cataclysm.exe's etgEffectCodeLoad in the p-code emulator for a list of effect files,
// with file I/O, the heap and a few runtime helpers replaced by Java stubs, then dumps the
// resulting effect statics in a normalised text form for comparison with hwc-fx.
public class EtgDump extends GhidraScript {
    static final String DIR = "D:\\Git\\HWC_Modern\\extracted\\etg";
    static final String LIST = "C:\\Users\\chris\\AppData\\Local\\Temp\\claude\\D--Git-HWC-Modern\\cb94e79d-33b9-4eaf-85d8-374ede201d93\\scratchpad\\fx\\ebg_list2.txt";
    static final String OUT = "C:\\Users\\chris\\AppData\\Local\\Temp\\claude\\D--Git-HWC-Modern\\cb94e79d-33b9-4eaf-85d8-374ede201d93\\scratchpad\\fx\\etg_dump.txt";
    static final long SENTINEL = 0xdeadbeefL;
    EmulatorHelper emu;
    long heap = 0x20000000L;
    long ptd = 0x1f000000L;
    byte[] file; int filePos;
    List<String> textures = new ArrayList<>();
    int meshCount = 0;

    long rd(long a) throws Exception {
        byte[] b = emu.readMemory(toAddr(a), 4);
        return (b[0] & 0xffL) | ((b[1] & 0xffL) << 8) | ((b[2] & 0xffL) << 16) | ((b[3] & 0xffL) << 24);
    }
    void wr(long a, long v) throws Exception {
        emu.writeMemory(toAddr(a), new byte[]{(byte) v, (byte) (v >> 8), (byte) (v >> 16), (byte) (v >> 24)});
    }
    String cstr(long a) throws Exception {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < 4096; i++) {
            byte c = emu.readMemory(toAddr(a + i), 1)[0];
            if (c == 0) break;
            sb.append((char) (c & 0xff));
        }
        return sb.toString();
    }
    long reg(String r) { return emu.readRegister(r).longValue() & 0xffffffffL; }
    void ret(long eax) throws Exception {
        long esp = reg("ESP");
        long ra = rd(esp);
        emu.writeRegister("EAX", eax);
        emu.writeRegister("ESP", esp + 4);
        emu.writeRegister(emu.getPCRegister(), ra);
    }
    long arg(int i) throws Exception { return rd(reg("ESP") + 4 + 4 * i); }

    File findCi(File dir, String name) {
        for (File f : dir.listFiles()) if (f.getName().equalsIgnoreCase(name)) return f;
        return null;
    }

    boolean stub(long pc) throws Exception {
        if (pc == 0x475550L) { // fileOpen(name, flags)
            String name = cstr(arg(0));
            String base = name.substring(name.lastIndexOf('\\') + 1);
            File f = findCi(new File(DIR), base);
            if (f == null) throw new Exception("missing " + name);
            file = Files.readAllBytes(f.toPath());
            filePos = 0;
            ret(1);
        } else if (pc == 0x4760b0L) { // fileLineRead(handle, dest, n)
            long dest = arg(1);
            int n = (int) arg(2);
            if (filePos >= file.length) { ret(0xfffffff4L); return true; }
            ByteArrayOutputStream o = new ByteArrayOutputStream();
            while (o.size() < n - 1 && filePos < file.length) {
                int ch = file[filePos++] & 0xff;
                if (ch == 13) {
                    if (filePos < file.length && file[filePos] == 10) filePos++;
                    break;
                }
                o.write(ch);
            }
            o.write(0);
            emu.writeMemory(toAddr(dest), o.toByteArray());
            ret(o.size() - 1);
        } else if (pc == 0x475e20L) { ret(0);
        } else if (pc == 0x4a9530L) { // memAllocFunction(size, flags)
            long size = arg(0);
            long p = heap;
            heap += (size + 15) & ~7L;
            emu.writeMemory(toAddr(p), new byte[(int) size]);
            ret(p);
        } else if (pc == 0x4a9780L) { ret(0);
        } else if (pc == 0x6714faL) { // _CIfmod: ST0 = fmod(ST1, ST0), one pop
            double y = x80(emu.readRegister("ST0")), x = x80(emu.readRegister("ST1"));
            double r = x % y;
            for (int k = 0; k < 7; k++) emu.writeRegister("ST" + k, emu.readRegister("ST" + (k + 1)));
            emu.writeRegister("ST0", to80(r));
            ret(reg("EAX"));
        } else if (pc == 0x66b650L) { ret(sscanf());
        } else if (pc == 0x66b450L) { // sprintf(dest, fmt, double)
            String fmt = cstr(arg(1));
            long lo = arg(2), hi = arg(3);
            double d = Double.longBitsToDouble((hi << 32) | lo);
            String r = fmt.equals("%f") ? String.format(java.util.Locale.ROOT, "%f", d) : String.format(java.util.Locale.ROOT, "%12.12f", d);
            emu.writeMemory(toAddr(arg(0)), (r + "\0").getBytes("ISO-8859-1"));
            ret(r.length());
        } else if (pc == 0x4a98f0L) { // memStringDupe(s)
            byte[] str = (cstr(arg(0)) + "\0").getBytes("ISO-8859-1");
            long p = heap;
            heap += (str.length + 15) & ~7L;
            emu.writeMemory(toAddr(p), str);
            ret(p);
        } else if (pc == 0x5800e0L) { // trTextureRegister(name, ...)
            String name = cstr(arg(0));
            int idx = -1;
            for (int i = 0; i < textures.size(); i++) if (textures.get(i).equalsIgnoreCase(name)) idx = i;
            if (idx < 0) { textures.add(name); idx = textures.size() - 1; }
            ret(idx);
        } else if (pc == 0x4aa2e0L) { ret(++meshCount);
        } else if (pc == 0x67aba0L) { ret(ptd);
        } else if (pc == 0x48a2a0L) { ret(0);
        } else if (pc == 0x45e030L || pc == 0x45e080L) {
            throw new Exception("dbgFatal: " + cstr(arg(2)) + " / " + cstr(arg(0)));
        } else return false;
        return true;
    }

    static boolean sp(int c) { return c == 32 || c == 9 || c == 10 || c == 13 || c == 11 || c == 12; }

    long sscanf() throws Exception {
        String in = cstr(arg(0));
        String fmt = cstr(arg(1));
        int ai = 2, i = 0, n = 0;
        for (int f = 0; f < fmt.length(); f++) {
            char fc = fmt.charAt(f);
            if (sp(fc)) { while (i < in.length() && sp(in.charAt(i))) i++; continue; }
            if (fc != '%') { if (i < in.length() && in.charAt(i) == fc) { i++; continue; } return n; }
            char t = fmt.charAt(++f);
            while (i < in.length() && sp(in.charAt(i))) i++;
            if (i >= in.length()) return n == 0 ? 0xffffffffL : n;
            if (t == 'd') {
                int j = i; boolean neg = false;
                if (in.charAt(j) == '+' || in.charAt(j) == '-') { neg = in.charAt(j) == '-'; j++; }
                int st = j; long v = 0;
                while (j < in.length() && Character.isDigit(in.charAt(j))) { v = (v * 10 + (in.charAt(j) - '0')) & 0xffffffffL; j++; }
                if (j == st) return n;
                if (neg) v = (-v) & 0xffffffffL;
                wr(arg(ai++), v); n++; i = j;
            } else if (t == 'f') {
                int j = i;
                if (in.charAt(j) == '+' || in.charAt(j) == '-') j++;
                int digits = 0;
                while (j < in.length() && Character.isDigit(in.charAt(j))) { j++; digits++; }
                if (j < in.length() && in.charAt(j) == '.') { j++; while (j < in.length() && Character.isDigit(in.charAt(j))) { j++; digits++; } }
                if (digits == 0) return n;
                int end = j;
                if (j < in.length() && (in.charAt(j) == 'e' || in.charAt(j) == 'E')) {
                    int k = j + 1;
                    if (k < in.length() && (in.charAt(k) == '+' || in.charAt(k) == '-')) k++;
                    int ds = k;
                    while (k < in.length() && Character.isDigit(in.charAt(k))) k++;
                    if (k > ds) end = k;
                }
                double d = Double.parseDouble(in.substring(i, end));
                wr(arg(ai++), Float.floatToRawIntBits((float) d) & 0xffffffffL); n++; i = end;
            } else if (t == 's') {
                int j = i;
                while (j < in.length() && !sp(in.charAt(j))) j++;
                emu.writeMemory(toAddr(arg(ai++)), (in.substring(i, j) + "\0").getBytes("ISO-8859-1"));
                n++; i = j;
            } else throw new Exception("sscanf format " + fmt);
        }
        return n;
    }

    static double x80(java.math.BigInteger b) {
        long mant = b.longValue();
        int se = b.shiftRight(64).intValue() & 0xffff;
        if (mant == 0) return (se & 0x8000) != 0 ? -0.0 : 0.0;
        int e = (se & 0x7fff) - 16383;
        double v = (mant >>> 11) * Math.pow(2, e - 52);
        return (se & 0x8000) != 0 ? -v : v;
    }
    static java.math.BigInteger to80(double d) {
        if (d == 0) return java.math.BigInteger.valueOf(Double.doubleToRawLongBits(d) < 0 ? 0x8000 : 0).shiftLeft(64);
        long bits = Double.doubleToRawLongBits(d);
        int sign = (int) (bits >>> 63);
        int e = (int) ((bits >>> 52) & 0x7ff);
        long m = bits & 0xfffffffffffffL;
        if (e == 0) { while ((m & (1L << 52)) == 0) { m <<= 1; e--; } e++; m &= 0xfffffffffffffL; }
        long mant = (1L << 63) | (m << 11);
        int se = (sign << 15) | (e - 1023 + 16383);
        return java.math.BigInteger.valueOf(se).shiftLeft(64).or(new java.math.BigInteger(Long.toUnsignedString(mant)));
    }

    long[] STUBS = {0x6714faL, 0x66b650L, 0x66b450L, 0x4a98f0L, 0x475550L, 0x4760b0L, 0x475e20L, 0x4a9530L, 0x4a9780L, 0x5800e0L, 0x4aa2e0L, 0x67aba0L, 0x48a2a0L, 0x45e030L, 0x45e080L};

    void load(String path, ConsoleTaskMonitor mon) throws Exception {
        long stack = 0x0ff00000L;
        emu.writeMemory(toAddr(0x0fe00000L), (path + "\0").getBytes("ISO-8859-1"));
        wr(stack, SENTINEL);
        wr(stack + 4, 0x0fe00000L);
        emu.writeRegister("ESP", stack);
        emu.writeRegister("EBP", stack + 0x1000);
        emu.writeRegister(emu.getPCRegister(), 0x467080L);
        long steps = 0;
        java.util.Set<Long> stubs = new java.util.HashSet<>();
        for (long x : STUBS) stubs.add(x);
        while (true) {
            long pc = emu.getExecutionAddress().getOffset();
            if (pc == SENTINEL) break;
            if (stubs.contains(pc)) { stub(pc); steps++; continue; }
            boolean ok = emu.run(mon);
            long npc = emu.getExecutionAddress().getOffset();
            if (!ok && npc != SENTINEL && !stubs.contains(npc)) {
                long esp = reg("ESP");
                StringBuilder st = new StringBuilder();
                for (int i = -4; i < 24; i++) st.append(" " + tok(rd(esp + 4 * i)));
                throw new Exception("fault at " + Long.toHexString(npc) + ": " + emu.getLastError() + " in " + path + " stack:" + st);
            }
        }
        totalSteps += steps;
    }
    long totalSteps = 0;
    PrintWriter log;

    Map<Long, String> fnames = new HashMap<>();

    String tok(long v) { return String.format("%08x", v); }

    protected void run() throws Exception {
        emu = new EmulatorHelper(currentProgram);
        ConsoleTaskMonitor mon = new ConsoleTaskMonitor();
        byte[] z = new byte[0x10000];
        for (long a = 0x8d4000L; a < 0xafa000L; a += 0x10000) emu.writeMemory(toAddr(a), z);
        emu.writeMemory(toAddr(ptd), new byte[0x400]);
        wr(0x870b04L, 0xffffffffL);
        for (long s : STUBS) emu.setBreakpoint(toAddr(s));
        emu.setBreakpoint(toAddr(SENTINEL));
        // Function names for the code dump: first script name in the ETG table per address.
        for (long e = 0x86f6d0L; e <= 0x8708f8L; e += 28) {
            long fn = rd(e + 4);
            if (!fnames.containsKey(fn)) fnames.put(fn, cstr(rd(e)));
        }
        fnames.put(0x4df030L, "SetTrHandle");
        fnames.put(0x4dde50L, "ModifyTexture");
        fnames.put(0x4df020L, "SetMeshdata");
        fnames.put(0x46c1d0L, "SpawnNewEffect");
        fnames.put(0x46ba60L, "CreateEffects");
        fnames.put(0x469910L, "CreateCallbackSetup");
        String[] sa = getScriptArgs();
        List<String> files = Files.readAllLines(Paths.get(sa.length > 0 ? sa[0] : LIST));
        log = new PrintWriter(new FileWriter(OUT + ".log"));
        PrintWriter out = new PrintWriter(new FileWriter(sa.length > 1 ? sa[1] : OUT));
        long t0 = System.currentTimeMillis();
        for (String f : files) {
            if (f.trim().isEmpty()) continue;
            out.println("L " + f.trim());
            load("ETG\\" + f.trim(), mon);
            log.println("done " + f + " steps " + totalSteps + " t " + (System.currentTimeMillis() - t0)); log.flush();
        }
        println("loaded " + files.size() + " in " + (System.currentTimeMillis() - t0) + " ms");
        for (long e = 0xae73a0L; e < 0xae7ba0L; e += 8) {
            long s = rd(e);
            if (s == 0) continue;
            dump(out, s);
        }
        for (int i = 0; i < textures.size(); i++) out.println("T " + i + " " + textures.get(i));
        out.close();
    }

    void dump(PrintWriter out, long s) throws Exception {
        long[] w = new long[27];
        for (int i = 0; i < 27; i++) w[i] = rd(s + 4 * i);
        String name = cstr(w[0]);
        out.println("S " + name + " npb=" + tok(w[2]) + " vs=" + tok(w[3]) + " rs=" + tok(w[4]) + " es=" + tok(w[5])
            + " nhist=" + tok(w[23]) + " sops=" + tok(w[26]) + " ndec=" + tok(w[11]) + " clen=" + tok(w[10]));
        if (w[2] == 0xffffffffL) return;
        StringBuilder sb = new StringBuilder("I");
        for (int i = 0; i < (int) w[8]; i++) {
            long idx = emu.readMemory(toAddr(w[6] + i), 1)[0] & 0xff;
            sb.append(" " + idx + "=" + tok(rd(w[7] + 4 * i)));
        }
        out.println(sb);
        sb = new StringBuilder("C ");
        for (long i = 0; i < w[10]; i += 4) sb.append(tok(rd(w[9] + i)) + " ");
        out.println(sb);
        sb = new StringBuilder("D");
        long nalt = 0;
        for (int i = 0; i < (int) w[11]; i++) {
            long d = w[12] + 12 * i;
            sb.append(" " + rd(d) + ":" + rd(d + 4) + ":" + rd(d + 8));
            nalt += rd(d + 8);
        }
        out.println(sb);
        for (int b = 0; b < 3; b++) {
            long code = w[14 + 3 * b], len = w[15 + 3 * b];
            sb = new StringBuilder("B" + b);
            long at = 0;
            while (at < len) {
                long op = rd(code + at);
                long n;
                if (op == 0x48) {
                    long np = rd(code + at + 8);
                    n = 5 + 2 * np;
                    for (int k = 0; k < n; k++) {
                        long v = rd(code + at + 4 * k);
                        if (k == 1) sb.append(" F:" + fnames.getOrDefault(v, "?" + tok(v)));
                        else if (k == 6 && (rd(code + at + 4) == 0x46c1d0L || rd(code + at + 4) == 0x46ba60L))
                            sb.append(" E:" + cstr(rd(v)));
                        else sb.append(" " + tok(v));
                    }
                    at += 4 * n;
                    continue;
                }
                n = rd(0x86f268L + 8 * op) / 4;
                if (n == 0) { sb.append(" BAD" + tok(op)); break; }
                for (int k = 0; k < n; k++) {
                    long v = rd(code + at + 4 * k);
                    if (op == 0x49 && k == 4) v = (v - w[13]) / 4;
                    sb.append(" " + tok(v));
                }
                at += 4 * n;
            }
            out.println(sb);
        }
        // Alternate offset table (total length from the decisions).
        sb = new StringBuilder("A");
        for (long i = 0; i < nalt; i++) sb.append(" " + tok(rd(w[13] + 4 * i)));
        out.println(sb);
    }
}
