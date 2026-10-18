// Clears a wrong no-return flag on the given functions, then resumes disassembly after every call to
// them and re-computes the calling functions' bodies so the code Ghidra cut off is reattached.
// Args: function entry addresses (hex).
import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.cmd.function.CreateFunctionCmd;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

import java.util.*;

public class FixNoReturn extends GhidraScript {
    @Override
    protected void run() throws Exception {
        Listing listing = currentProgram.getListing();
        Set<Function> callers = new HashSet<>();
        int resumed = 0;
        for (String a : getScriptArgs()) {
            Function f = getFunctionAt(toAddr(a));
            if (f == null) { println("no function at " + a); continue; }
            f.setNoReturn(false);
            for (Reference r : getReferencesTo(f.getEntryPoint())) {
                if (!r.getReferenceType().isCall()) continue;
                Instruction ins = listing.getInstructionAt(r.getFromAddress());
                if (ins == null) continue;
                ins.setFlowOverride(ghidra.program.model.listing.FlowOverride.NONE);
                Address next = ins.getMaxAddress().next();
                if (next != null && listing.getInstructionAt(next) == null) {
                    listing.clearCodeUnits(next, next, false);
                    new DisassembleCommand(next, null, true).applyTo(currentProgram, monitor);
                    resumed++;
                }
                Function c = getFunctionContaining(r.getFromAddress());
                if (c != null) callers.add(c);
            }
        }
        int fixedBodies = 0;
        for (Function c : callers) {
            if (CreateFunctionCmd.fixupFunctionBody(currentProgram, c, monitor)) fixedBodies++;
        }
        println("resumed " + resumed + " call sites, re-bodied " + fixedBodies + " of " + callers.size() + " callers");
    }
}
