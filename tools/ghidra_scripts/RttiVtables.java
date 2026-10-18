// MSVC RTTI walk: for every type descriptor string (.?AV/.?AU), find its CompleteObjectLocators,
// the vftables they head, the vftable slots (function entry points), and the functions whose code
// references each vftable (constructors / destructors write the vfptr).
// Output TSV lines: VT <class> <colOffset> <vtable> <slot0,slot1,...> <codeRefFuncs>
// Arg 0: output path.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.util.*;

public class RttiVtables extends GhidraScript {
    static String demangle(String s) {
        // .?AVname@ns1@ns2@@ -> ns2::ns1::name ; templates left mangled
        String body = s.substring(4);
        if (body.endsWith("@@")) body = body.substring(0, body.length() - 2);
        if (body.startsWith("?$")) return body;
        String[] parts = body.split("@");
        StringBuilder b = new StringBuilder();
        for (int i = parts.length - 1; i >= 0; i--) { if (b.length() > 0) b.append("::"); b.append(parts[i]); }
        return b.toString();
    }

    List<Address> refsTo(Memory mem, Address target) throws Exception {
        // brute-force scan of initialized data for the 4-byte little-endian pointer value
        List<Address> out = new ArrayList<>();
        int v = (int) target.getOffset();
        byte[] pat = new byte[] {(byte) v, (byte) (v >> 8), (byte) (v >> 16), (byte) (v >> 24)};
        for (MemoryBlock b : mem.getBlocks()) {
            if (!b.isInitialized() || b.isExecute()) continue;
            Address p = b.getStart();
            while (p != null && p.compareTo(b.getEnd()) <= 0) {
                Address h = mem.findBytes(p, b.getEnd(), pat, null, true, monitor);
                if (h == null) break;
                if (h.getOffset() % 4 == 0) out.add(h);
                p = h.add(1);
            }
        }
        return out;
    }

    @Override
    protected void run() throws Exception {
        String outPath = getScriptArgs()[0];
        Memory mem = currentProgram.getMemory();
        Listing listing = currentProgram.getListing();
        ReferenceManager rm = currentProgram.getReferenceManager();
        try (PrintWriter w = new PrintWriter(new OutputStreamWriter(new FileOutputStream(outPath), "UTF-8"))) {
            for (MemoryBlock blk : mem.getBlocks()) {
                if (!blk.isInitialized() || blk.isExecute()) continue;
                Address p = blk.getStart();
                while (p != null && p.compareTo(blk.getEnd()) < 0) {
                    Address hit = mem.findBytes(p, blk.getEnd(), ".?A".getBytes(), null, true, monitor);
                    if (hit == null) break;
                    StringBuilder s = new StringBuilder();
                    Address q = hit;
                    for (int i = 0; i < 400; i++) { byte c = mem.getByte(q); if (c == 0) break; s.append((char) c); q = q.add(1); }
                    p = q.add(1);
                    String cls = demangle(s.toString());
                    Address td = hit.subtract(8);
                    for (Address r : refsTo(mem, td)) {
                        Address col = r.subtract(12);
                        if (mem.getInt(col) != 0) continue;
                        int off = mem.getInt(col.add(4));
                        for (Address m : refsTo(mem, col)) {
                            Address vt = m.add(4);
                            List<String> slots = new ArrayList<>();
                            Address sl = vt;
                            for (int i = 0; i < 200; i++) {
                                if (i > 0 && rm.getReferenceCountTo(sl) > 0) break;
                                long val = mem.getInt(sl) & 0xffffffffL;
                                Address fa;
                                try { fa = toAddr(val); } catch (Exception e) { break; }
                                MemoryBlock fb = mem.getBlock(fa);
                                if (fb == null || !fb.isExecute()) break;
                                slots.add(fa.toString());
                                sl = sl.add(4);
                            }
                            Set<String> code = new TreeSet<>();
                            for (Reference cr : rm.getReferencesTo(vt)) {
                                Function f = getFunctionContaining(cr.getFromAddress());
                                if (f != null) code.add(f.getEntryPoint().toString());
                            }
                            w.println("VT\t" + cls + "\t" + off + "\t" + vt + "\t" + String.join(",", slots) + "\t" + String.join(",", code));
                        }
                    }
                }
            }
        }
        println("done");
    }
}
