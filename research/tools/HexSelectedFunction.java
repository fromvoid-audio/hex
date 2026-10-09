// Original bounded research helper; not shipped application code.
// @category Hex.Research
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import java.util.Arrays;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;

public class HexSelectedFunction extends GhidraScript {
    @Override public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 2) throw new IllegalArgumentException("address and create-new private output required");
        Address address = toAddr(args[0]);
        if (address == null || !currentProgram.getMemory().contains(address))
            throw new IllegalArgumentException("address outside imported program");
        if (!disassemble(address)) throw new IllegalStateException("selected disassembly failed");
        Function function = getFunctionAt(address);
        if (function == null) function = createFunction(address, null);
        if (function == null) throw new IllegalStateException("selected function not established");
        DecompInterface decompiler = new DecompInterface();
        try {
            if (!decompiler.openProgram(currentProgram)) throw new IllegalStateException(decompiler.getLastMessage());
            DecompileResults result = decompiler.decompileFunction(function, 30, monitor);
            if (!result.decompileCompleted() || result.getDecompiledFunction() == null)
                throw new IllegalStateException("decompile failed: " + result.getErrorMessage());
            Instruction entry = currentProgram.getListing().getInstructionAt(address);
            String output = "entry_instruction=" + entry + "\nentry_flows="
                + (entry == null ? "none" : Arrays.toString(entry.getFlows())) + "\nprogram=" + currentProgram.getName() + "\nlanguage="
                + currentProgram.getLanguageID() + "\nentry=" + function.getEntryPoint()
                + "\nfunction=" + function.getName() + "\nbody=" + function.getBody()
                + "\nwarning=" + result.getErrorMessage() + "\n\n"
                + result.getDecompiledFunction().getC();
            Files.writeString(Path.of(args[1]), output, StandardCharsets.UTF_8,
                StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE);
            println("hex selected function exported at " + function.getEntryPoint()
                + "; body bytes=" + function.getBody().getNumAddresses());
        } finally { decompiler.dispose(); }
    }
}
