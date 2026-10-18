// Library survey: for every function in the given address ranges, dump TSV:
// entry, name, size, #callers from outside the ranges (game code), callees, referenced strings.
// Also dumps all RTTI type-descriptor strings (.?AV / .?AU) and their addresses.
// Args: outPath lo1 hi1 [lo2 hi2 ...]
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.data.StringDataInstance;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.util.*;

public class LibSurvey extends GhidraScript {
    static String esc(String s) {
        StringBuilder b = new StringBuilder();
        for (char c : s.toCharArray()) {
            if (c == '\t' || c == '\n' || c == '\r') b.append(' ');
            else if (c < 0x20 || c > 0x7e) b.append('?');
            else b.append(c);
        }
        return b.length() > 80 ? b.substring(0, 80) : b.toString();
    }

    @Override
    protected void run() throws Exception {
        String[] a = getScriptArgs();
        String out = a[0];
        AddressSet set = new AddressSet();
        for (int i = 1; i + 1 < a.length; i += 2)
            set.addRange(toAddr(a[i]), toAddr(a[i + 1]).subtract(1));
        Listing listing = currentProgram.getListing();
        int n = 0;
        try (PrintWriter w = new PrintWriter(new OutputStreamWriter(new FileOutputStream(out), "UTF-8"))) {
            for (Function f : listing.getFunctions(set, true)) {
                Set<String> callees = new LinkedHashSet<>();
                Set<String> strs = new LinkedHashSet<>();
                for (Instruction ins : listing.getInstructions(f.getBody(), true)) {
                    for (Reference r : ins.getReferencesFrom()) {
                        Address to = r.getToAddress();
                        if (r.getReferenceType().isCall()) {
                            Function c = getFunctionAt(to);
                            if (c != null) callees.add(c.getName());
                        } else {
                            Data d = listing.getDataAt(to);
                            if (d != null && d.hasStringValue()) strs.add(esc(String.valueOf(d.getValue())));
                            else if (d != null && d.isPointer() && d.getValue() instanceof Address) {
                                Data d2 = listing.getDataAt((Address) d.getValue());
                                if (d2 != null && d2.hasStringValue()) strs.add("*" + esc(String.valueOf(d2.getValue())));
                            }
                        }
                    }
                }
                int ext = 0, inr = 0;
                Set<String> extCallers = new TreeSet<>();
                for (Reference r : getReferencesTo(f.getEntryPoint())) {
                    if (set.contains(r.getFromAddress())) inr++;
                    else {
                        ext++;
                        Function cf = getFunctionContaining(r.getFromAddress());
                        if (cf != null && extCallers.size() < 6) extCallers.add(cf.getName());
                    }
                }
                w.println(f.getEntryPoint() + "\t" + f.getName() + "\t" + f.getBody().getNumAddresses() + "\t" + ext + "\t" + inr
                        + "\t" + String.join(",", extCallers) + "\t" + String.join(",", callees) + "\t" + String.join(" | ", strs));
                n++;
            }
            // RTTI strings
            Memory mem = currentProgram.getMemory();
            for (MemoryBlock b : mem.getBlocks()) {
                if (!b.isInitialized()) continue;
                Address p = b.getStart();
                while (p != null && p.compareTo(b.getEnd()) < 0) {
                    Address hit = mem.findBytes(p, b.getEnd(), ".?A".getBytes(), null, true, monitor);
                    if (hit == null) break;
                    StringBuilder s = new StringBuilder();
                    Address q = hit;
                    for (int i = 0; i < 300; i++) { byte c = mem.getByte(q); if (c == 0) break; s.append((char) c); q = q.add(1); }
                    w.println("RTTI\t" + hit + "\t" + esc(s.toString()));
                    p = q.add(1);
                }
            }
        }
        println("functions " + n);
    }
}
