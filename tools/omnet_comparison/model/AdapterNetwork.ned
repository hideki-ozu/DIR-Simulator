// Test-only composition. All bus, arbitration and wire timing remain in FiCo.
package dir.omnetcomparison;

import fico4omnet.bus.can.CanBus;
import fico4omnet.buffer.can.CanOutputBuffer;
import fico4omnet.linklayer.can.CanNodePort;

simple AdapterRecorder
{
    parameters:
        @class(DirAdapterRecorder);
}

simple AdapterSource
{
    parameters:
        @class(DirAdapterSource);
}

simple AdapterSink
{
    parameters:
        @class(DirAdapterSink);
    gates:
        input directIn @directIn;
}

simple AdapterOutputBuffer extends CanOutputBuffer
{
    parameters:
        @class(DirAdapterOutputBuffer);
        MOB = false;
}

module AdapterNode
{
    parameters:
        @node;
        string nodeLabel;
        string sourceFile;
        int queueCapacity = default(16);
        int txProcessingPs = default(0);
        int rxProcessingPs = default(0);
        int txChannelPs = default(0);
        int rxChannelPs = default(0);
        string rxFilter = default("*");
        int errorperc = 0;
    gates:
        inout gate;
    submodules:
        canNodePort: CanNodePort;
        bufferOut: AdapterOutputBuffer {
            parameters:
                destination_gates = "canPortOutput.directIn";
        }
        source: AdapterSource;
        sink: AdapterSink;
    connections allowunconnected:
        canNodePort.phygate <--> gate;
}

network AdapterNetwork
{
    parameters:
        int nodeCount = default(3);
        int bitrate = default(500000);
        int horizonPs;
        string outputFile;
        string routingFile = default("");
    submodules:
        recorder: AdapterRecorder;
        bus: CanBus {
            parameters:
                bandwidth = parent.bitrate * 1bps;
                bitStuffingPercentage = 0;
            gates:
                gate[parent.nodeCount];
        }
        node[nodeCount]: AdapterNode;
    connections:
        for i=0..nodeCount-1 {
            node[i].gate <--> bus.gate[i];
        }
}
