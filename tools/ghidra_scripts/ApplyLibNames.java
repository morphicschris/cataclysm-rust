// Apply function names from a TSV (address \t name [\t source]). A name of the form "A::B::m"
// puts the function into namespace A::B (created if missing) with simple name m.
// Only renames functions whose current name is an auto name (FUN_*, thunk_FUN_*), and only inside
// the ranges given. Args: tsvPath lo1 hi1 [lo2 hi2 ...]
import ghidra.app.script.GhidraScript;
import ghidra.app.util.NamespaceUtils;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.nio.file.*;
import java.util.*;

public class ApplyLibNames extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] a = getScriptArgs();
        AddressSet set = new AddressSet();
        for (int i = 1; i + 1 < a.length; i += 2) set.addRange(toAddr(a[i]), toAddr(a[i + 1]).subtract(1));
        int ok = 0, skip = 0, fail = 0;
        for (String line : Files.readAllLines(Paths.get(a[0]))) {
            String[] p = line.split("\t");
            if (p.length < 2) continue;
            Address ad = toAddr(p[0]);
            if (!set.contains(ad)) { skip++; continue; }
            Function f = getFunctionAt(ad);
            if (f == null) { skip++; continue; }
            String cur = f.getName();
            boolean mine = f.getName(true).equals(p[1]) || cur.equals(p[1]);
            if (!(cur.startsWith("FUN_") || cur.startsWith("thunk_FUN_") || mine)) { skip++; continue; }
            if (cur.equals(p[1]) && f.getParentNamespace().isGlobal()) { skip++; continue; }
            try {
                f.getSymbol().setNameAndNamespace(p[1], currentProgram.getGlobalNamespace(), SourceType.ANALYSIS);
                ok++;
            } catch (Exception e) {
                fail++;
                println("fail " + p[0] + " " + p[1] + ": " + e.getMessage());
            }
        }
        println("renamed " + ok + " skipped " + skip + " failed " + fail);
    }
}
