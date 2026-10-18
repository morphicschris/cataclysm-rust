// For every function in the library ranges, count references coming from the game-code ranges.
// Output TSV: entry, name, refsFromGame, distinctGameCallers, sample callers.
// Args: outPath gameLo1 gameHi1 gameLo2 gameHi2 -- libLo1 libHi1 [libLo2 libHi2 ...]
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.util.*;

public class GameToLibCalls extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] a = getScriptArgs();
        AddressSet game = new AddressSet(), lib = new AddressSet();
        int i = 1;
        for (; i < a.length && !a[i].equals("--"); i += 2) game.addRange(toAddr(a[i]), toAddr(a[i + 1]).subtract(1));
        for (i++; i + 1 < a.length; i += 2) lib.addRange(toAddr(a[i]), toAddr(a[i + 1]).subtract(1));
        try (PrintWriter w = new PrintWriter(new OutputStreamWriter(new FileOutputStream(a[0]), "UTF-8"))) {
            for (Function f : currentProgram.getListing().getFunctions(lib, true)) {
                int n = 0;
                Set<String> callers = new TreeSet<>();
                for (Reference r : getReferencesTo(f.getEntryPoint())) {
                    if (!game.contains(r.getFromAddress())) continue;
                    n++;
                    Function c = getFunctionContaining(r.getFromAddress());
                    callers.add(c == null ? r.getFromAddress().toString() : c.getName());
                }
                if (n == 0) continue;
                List<String> l = new ArrayList<>(callers);
                w.println(f.getEntryPoint() + "\t" + f.getName() + "\t" + n + "\t" + callers.size() + "\t"
                        + String.join(",", l.subList(0, Math.min(8, l.size()))));
            }
        }
    }
}
