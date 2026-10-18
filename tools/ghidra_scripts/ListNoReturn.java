// Lists functions marked non-returning, with how many call sites are followed by undefined bytes
// (a sign Ghidra wrongly decided the callee never returns and cut the caller short).
// Arg 0: "fix" to clear the no-return flag on functions where most call sites look wrong.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class ListNoReturn extends GhidraScript {
    @Override
    protected void run() throws Exception {
        boolean fix = getScriptArgs().length > 0 && getScriptArgs()[0].equals("fix");
        Listing listing = currentProgram.getListing();
        int fixed = 0;
        for (Function f : listing.getFunctions(true)) {
            if (!f.hasNoReturn()) continue;
            int calls = 0, cut = 0;
            for (Reference r : getReferencesTo(f.getEntryPoint())) {
                if (!r.getReferenceType().isCall()) continue;
                calls++;
                Instruction ins = listing.getInstructionAt(r.getFromAddress());
                if (ins == null) continue;
                Address next = ins.getMaxAddress().next();
                if (next != null && listing.getInstructionAt(next) == null && listing.getDefinedDataAt(next) == null) {
                    byte b = getByte(next);
                    if (b != (byte) 0xcc && b != (byte) 0x90) cut++;
                }
            }
            boolean wrong = calls > 0 && cut * 2 > calls && !f.getName().matches("(?i).*(exit|abort|fatal|terminate|longjmp|raise|throw).*");
            println(String.format("%s %s calls=%d cut=%d %s", f.getEntryPoint(), f.getName(), calls, cut, wrong ? "LIKELY-WRONG" : ""));
            if (fix && wrong) { f.setNoReturn(false); fixed++; }
        }
        println("fixed " + fixed);
    }
}
