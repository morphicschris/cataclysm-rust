// Dumps compiler-robust features for every function in the current program as JSON lines:
// address, name, size, callees (by name), string literals referenced, scalar constants (>= 0x100 or
// float bit patterns), number of basic blocks. Used to match cataclysm.exe against an HW1 build made
// with a different compiler, where opcode hashes don't line up.
// Arg 0: output path.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.block.BasicBlockModel;
import ghidra.program.model.data.StringDataInstance;
import ghidra.program.model.listing.*;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.*;

import java.io.*;
import java.util.*;

public class DumpFunctionFeatures extends GhidraScript {
    private static String esc(String s) {
        StringBuilder b = new StringBuilder();
        for (char c : s.toCharArray()) {
            if (c == '"' || c == '\\') b.append('\\').append(c);
            else if (c < 0x20 || c > 0x7e) b.append(String.format("\\u%04x", (int) c));
            else b.append(c);
        }
        return b.toString();
    }

    @Override
    protected void run() throws Exception {
        String out = getScriptArgs().length > 0 ? getScriptArgs()[0] : "features.jsonl";
        Listing listing = currentProgram.getListing();
        ReferenceManager refs = currentProgram.getReferenceManager();
        BasicBlockModel bbm = new BasicBlockModel(currentProgram);
        int n = 0;
        try (PrintWriter w = new PrintWriter(new OutputStreamWriter(new FileOutputStream(out), "UTF-8"))) {
            for (Function f : listing.getFunctions(true)) {
                if (monitor.isCancelled()) break;
                if (f.isExternal() || f.isThunk()) continue;
                List<String> callees = new ArrayList<>();
                Set<String> strings = new TreeSet<>();
                List<Long> consts = new ArrayList<>();
                for (Instruction ins : listing.getInstructions(f.getBody(), true)) {
                    for (Reference r : ins.getReferencesFrom()) {
                        Address to = r.getToAddress();
                        if (r.getReferenceType().isCall()) {
                            Function c = getFunctionAt(to);
                            if (c != null) {
                                if (c.isThunk()) c = c.getThunkedFunction(true);
                                callees.add(c.getName());
                            }
                        } else if (r.getReferenceType().isData()) {
                            Data d = listing.getDataAt(to);
                            if (d != null && StringDataInstance.isString(d)) {
                                Object v = d.getValue();
                                if (v != null) strings.add(v.toString());
                            }
                        }
                    }
                    for (int op = 0; op < ins.getNumOperands(); op++) {
                        for (Object o : ins.getOpObjects(op)) {
                            if (o instanceof Scalar) {
                                long v = ((Scalar) o).getUnsignedValue();
                                // skip small ints, stack offsets and anything that is an address in the image
                                if (v >= 0x100 && v != 0xffffffffL && !currentProgram.getMemory().contains(toAddr(v)))
                                    consts.add(v);
                            }
                        }
                    }
                }
                int blocks = 0;
                var it = bbm.getCodeBlocksContaining(f.getBody(), monitor);
                while (it.hasNext()) { it.next(); blocks++; }
                StringBuilder b = new StringBuilder();
                b.append("{\"addr\":\"").append(f.getEntryPoint()).append("\",\"name\":\"").append(esc(f.getName()))
                 .append("\",\"size\":").append(f.getBody().getNumAddresses()).append(",\"blocks\":").append(blocks)
                 .append(",\"callees\":[");
                for (int i = 0; i < callees.size(); i++) b.append(i > 0 ? "," : "").append('"').append(esc(callees.get(i))).append('"');
                b.append("],\"strings\":[");
                int i = 0;
                for (String s : strings) b.append(i++ > 0 ? "," : "").append('"').append(esc(s)).append('"');
                b.append("],\"consts\":[");
                for (i = 0; i < consts.size(); i++) b.append(i > 0 ? "," : "").append(consts.get(i));
                b.append("]}");
                w.println(b);
                n++;
            }
        }
        println("wrote " + n + " functions to " + out);
    }
}
