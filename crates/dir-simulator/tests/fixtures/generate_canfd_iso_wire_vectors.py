"""Reproduce independent ISO vectors; requires pinned upstream checkout and g++."""
from pathlib import Path
import argparse
import hashlib
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--reference", required=True, type=Path,
                    help="checkout of pierremolinaro/canfd-plugin-for-saleae-logic-analyzer")
parser.add_argument("--output", required=True, type=Path)
args = parser.parse_args()
source_path = args.reference / "src/CANFDMolinaroSimulationDataGenerator.cpp"
expected_hash = "9da38614ddef670437c6fe59f91fe0d3876f186e012cf06f19981a186ce6ddd6"
if hashlib.sha256(source_path.read_bytes()).hexdigest() != expected_hash:
    raise SystemExit("Unexpected reference source SHA-256")
source = source_path.read_text()
workspace = tempfile.TemporaryDirectory(prefix="dir-canfd-reference-")
root = Path(workspace.name)
start=source.index('class CANFDFrameBitsGenerator {')
end=source.index('//  CANMolinaroSimulationDataGenerator',start)
body=source[start:end]
prefix='''#include <cstdint>
#include <iostream>
typedef enum {standardFrame, extendedFrame} FrameFormat;
typedef enum {DOMINANT_BIT, RECESSIVE_BIT} GeneratedBit;
typedef enum {CANFD_NON_ISO_PROTOCOL, CANFD_ISO_PROTOCOL} ProtocolSetting;
typedef enum {ACK_SLOT_DOMINANT, ACK_SLOT_RECESSIVE} AckSlot;
'''
main=r'''
int main() {
std::cout << "[";
bool comma=false;
for(unsigned dlc=0;dlc<16;dlc++) for(unsigned extended=0;extended<2;extended++) for(unsigned pattern=0;pattern<4;pattern++) {
 uint8_t data[64]={};
 unsigned length=CANFDFrameBitsGenerator::lengthForCode(dlc);
 for(unsigned i=0;i<length;i++) data[i]=pattern==0?0:pattern==1?255:pattern==2?85:uint8_t(i*37+13);
 uint32_t id=pattern==0?0:pattern==1?(extended?0x1fffffff:0x7ff):pattern==2?(extended?0x15555555:0x555):(extended?0x1234567:0x123);
 bool brs=pattern&1,esi=pattern&2;
 CANFDFrameBitsGenerator frame(id,extended?extendedFrame:standardFrame,CANFD_ISO_PROTOCOL,dlc,brs?RECESSIVE_BIT:DOMINANT_BIT,data,ACK_SLOT_DOMINANT,esi?RECESSIVE_BIT:DOMINANT_BIT);
 if(comma) {std::cout<<",\n";}
 comma=true;
 std::cout<<"{\"id\":"<<id<<",\"extended\":"<<(extended?"true":"false")<<",\"brs\":"<<(brs?"true":"false")<<",\"esi\":"<<(esi?"true":"false")<<",\"data\":[";
 for(unsigned i=0;i<length;i++){if(i)std::cout<<",";std::cout<<unsigned(data[i]);}
 unsigned data_bits=0;
 for(unsigned i=0;i<frame.frameLength()-3;i++) if(frame.dataBitRateAtIndex(i)) data_bits++;
 std::cout<<"],\"nominal_bits\":"<<(frame.frameLength()-3-data_bits)<<",\"data_bits\":"<<data_bits<<",\"crc\":"<<frame.frameCRC()<<",\"stuff_count\":"<<frame.stuffBitCount()<<",\"bits\":\"";
 // Upstream includes intermission; fixture comparison is SOF through EOF.
 for(unsigned i=0;i<frame.frameLength()-3;i++)std::cout<<(frame.bitAtIndex(i)?'1':'0');
 std::cout<<"\"}";
}
std::cout<<"]\n";
}
'''
(root/'reference_driver.cpp').write_text('/*\n' + (args.reference / 'LICENSE').read_text() + '*/\n' + prefix+body+main)
subprocess.run(['g++','-std=c++11','-Wall','-Wextra','-O2',str(root/'reference_driver.cpp'),'-o',str(root/'reference_driver')],check=True)
with args.output.open('w') as f: subprocess.run([str(root/'reference_driver')],stdout=f,check=True)
