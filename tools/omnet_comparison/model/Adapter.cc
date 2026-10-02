// Test-only FiCo adapter; no DIR model code or upstream implementations copied.
// Native completion includes IFG: native EOF and release are inseparable here.
#include <omnetpp.h>
#include "fico4omnet/buffer/can/CanOutputBuffer.h"
#include "fico4omnet/linklayer/can/CanPortInput.h"
#include "fico4omnet/linklayer/can/CanFrameTiming.h"

#include <algorithm>
#include <cctype>
#include <fstream>
#include <limits>
#include <set>
#include <sstream>
#include <string>
#include <vector>

using namespace omnetpp;
using FiCo4OMNeT::CanDataFrame;

namespace {
struct Request {
    int64_t generation;
    std::string id;
    bool extended;
    unsigned int canId;
    std::string hex;
    std::vector<uint8_t> bytes;
};

int64_t integer(const std::string& s, const char *field) {
    if (s.empty() || !std::all_of(s.begin(), s.end(), [](unsigned char c) { return std::isdigit(c); }))
        throw cRuntimeError("Invalid nonnegative integer for %s: %s", field, s.c_str());
    try { return std::stoll(s); }
    catch (const std::exception&) { throw cRuntimeError("Integer out of range for %s", field); }
}

std::vector<Request> readRequests(const char *path) {
    std::ifstream in(path);
    if (!in) throw cRuntimeError("Cannot read source TSV: %s", path);
    std::string line;
    if (!std::getline(in, line) || line != "generation_ps\trequest_id\tformat\tcan_id\tpayload_hex")
        throw cRuntimeError("Invalid source TSV header: %s", path);
    std::vector<Request> requests;
    std::set<std::string> ids;
    while (std::getline(in, line)) {
        if (line.empty()) continue;
        std::vector<std::string> fields;
        std::istringstream row(line);
        std::string value;
        while (std::getline(row, value, '\t')) fields.push_back(value);
        if (fields.size() != 5) throw cRuntimeError("Source TSV requires exactly 5 columns: %s", path);
        Request r;
        r.generation = integer(fields[0], "generation_ps");
        r.id = fields[1];
        if (r.id.empty() || !ids.insert(r.id).second) throw cRuntimeError("Empty or duplicate request_id: %s", path);
        if (fields[2] != "standard" && fields[2] != "extended") throw cRuntimeError("Unsupported CAN format");
        r.extended = fields[2] == "extended";
        auto id = integer(fields[3], "can_id");
        if (id > (r.extended ? 0x1fffffff : 0x7ff)) throw cRuntimeError("CAN ID exceeds format range");
        r.canId = static_cast<unsigned int>(id);
        r.hex = fields[4] == "-" ? "" : fields[4];
        if (r.hex.size() > 16 || r.hex.size() % 2 ||
            !std::all_of(r.hex.begin(), r.hex.end(), [](unsigned char c) { return std::isxdigit(c); }))
            throw cRuntimeError("Invalid Classical CAN payload_hex");
        std::transform(r.hex.begin(), r.hex.end(), r.hex.begin(), [](unsigned char c) { return std::tolower(c); });
        for (size_t i = 0; i < r.hex.size(); i += 2)
            r.bytes.push_back(static_cast<uint8_t>(std::stoul(r.hex.substr(i, 2), nullptr, 16)));
        requests.push_back(r);
    }
    return requests;
}

std::string csv(const std::string& s) {
    std::string result = "\"";
    for (char c : s) { result += c; if (c == '"') result += '"'; }
    return result + '"';
}

simtime_t ps(int64_t value) { return SimTime(value, SIMTIME_PS); }

std::string payloadHex(const CanDataFrame& frame) {
    constexpr char digits[] = "0123456789abcdef";
    std::string hex;
    hex.reserve(frame.getPayloadBytesArraySize() * 2);
    for (size_t i = 0; i < frame.getPayloadBytesArraySize(); ++i) {
        const auto byte = frame.getPayloadBytes(i);
        hex += digits[byte >> 4];
        hex += digits[byte & 0x0f];
    }
    return hex;
}
}

class DirAdapterRecorder : public cSimpleModule {
    std::ofstream out;
    cMessage *stop = nullptr;
public:
    int64_t horizon = 0;
    std::vector<std::vector<Request>> inputs;
    std::set<unsigned int> canIds;
    ~DirAdapterRecorder() override { cancelAndDelete(stop); }
    void record(const char *event, cModule *node, CanDataFrame *frame, int waiting = -1) {
        if (simTime() >= ps(horizon)) return;
        out << event << ',' << simTime().inUnit(SIMTIME_PS) << ','
            << csv(node->par("nodeLabel").stdstringValue()) << ','
            << csv(frame->par("request_id").stringValue()) << ','
            << csv(frame->par("source").stringValue()) << ','
            << (frame->getExtendedId() ? "extended" : "standard") << ',' << frame->getCanID() << ','
            << csv(payloadHex(*frame)) << ',' << frame->getBitLength() << ',';
        if (waiting >= 0) out << waiting;
        out << '\n';
        out.flush();
        if (!out) throw cRuntimeError("Writing adapter CSV failed");
    }
protected:
    void initialize() override {
        if (SimTime::getScaleExp() != -12) throw cRuntimeError("Adapter requires simtime-resolution=ps");
        cModule *network = getParentModule();
        horizon = network->par("horizonPs").intValue();
        int count = network->par("nodeCount").intValue();
        if (horizon < 0 || count < 2 || network->par("bitrate").intValue() <= 0)
            throw cRuntimeError("Invalid horizonPs, nodeCount or bitrate");
        out.open(network->par("outputFile").stringValue());
        if (!out) throw cRuntimeError("Cannot open adapter outputFile");
        out << "event,time_ps,node,request_id,source,format,can_id,payload_hex,native_bits,queue_waiting\n";
        out.flush();
        std::set<std::string> labels;
        for (int i = 0; i < count; ++i) {
            cModule *node = network->getSubmodule("node", i);
            if (!labels.insert(node->par("nodeLabel").stdstringValue()).second)
                throw cRuntimeError("Duplicate nodeLabel");
            for (const char *param : {"queueCapacity", "txProcessingPs", "rxProcessingPs", "txChannelPs", "rxChannelPs"})
                if (node->par(param).intValue() < 0) throw cRuntimeError("Negative node parameter %s", param);
            inputs.push_back(readRequests(node->par("sourceFile").stringValue()));
            for (const auto& r : inputs.back()) canIds.insert(r.canId);
        }
        stop = new cMessage("exclusive-horizon");
        stop->setSchedulingPriority(std::numeric_limits<short>::min());
        scheduleAt(ps(horizon), stop);
    }
    void handleMessage(cMessage *msg) override { endSimulation(); }
};
Define_Module(DirAdapterRecorder);

namespace {
DirAdapterRecorder *recorder(cModule *node) {
    return check_and_cast<DirAdapterRecorder *>(node->getParentModule()->getSubmodule("recorder"));
}
}

class DirAdapterOutputBuffer : public FiCo4OMNeT::CanOutputBuffer {
public:
    int queueWaiting() const { return frames.size() - (currentFrame ? 1 : 0); }
    void putFrame(cMessage *msg) override {
        auto frame = check_and_cast<CanDataFrame *>(msg);
        cModule *node = getParentModule();
        int waiting = queueWaiting();
        if (waiting >= node->par("queueCapacity").intValue()) {
            recorder(node)->record("dropped", node, frame, waiting);
            delete frame;
            return;
        }
        FiCo4OMNeT::CanOutputBuffer::putFrame(msg);
        recorder(node)->record("enqueued", node, frame, queueWaiting());
    }
    void receiveSendingPermission(unsigned int id, bool extended, bool rtr) override {
        FiCo4OMNeT::CanOutputBuffer::receiveSendingPermission(id, extended, rtr);
        recorder(getParentModule())->record("sof", getParentModule(), currentFrame, frames.size() - 1);
    }
    void sendingCompleted() override {
        recorder(getParentModule())->record("native_complete", getParentModule(), currentFrame, frames.size() - 1);
        FiCo4OMNeT::CanOutputBuffer::sendingCompleted();
    }
};
Define_Module(DirAdapterOutputBuffer);

class DirAdapterSource : public cSimpleModule {
    std::set<cMessage *> pending;
public:
    ~DirAdapterSource() override { for (auto msg : pending) cancelAndDelete(msg); }
protected:
    int numInitStages() const override { return 2; }
    void initialize(int stage) override {
        if (stage != 1) return;
        cModule *node = getParentModule();
        auto rec = recorder(node);
        for (const auto& r : rec->inputs.at(node->getIndex())) {
            if (r.generation >= rec->horizon) continue;
            auto frame = new CanDataFrame(r.id.c_str());
            frame->setCanID(r.canId);
            frame->setExtendedId(r.extended);
            frame->setDlc(r.bytes.size());
            frame->setPayloadLength(r.bytes.size());
            frame->setPayloadBytesArraySize(r.bytes.size());
            for (size_t i = 0; i < r.bytes.size(); ++i) frame->setPayloadBytes(i, r.bytes[i]);
            auto payload = new cPacket("payload-envelope");
            payload->setByteLength(r.bytes.size());
            frame->encapsulate(payload);
            frame->setBitLength(FiCo4OMNeT::CanFrameTiming::frameBitLength(*frame, 0));
            frame->addPar("request_id") = r.id.c_str();
            frame->addPar("source") = node->par("nodeLabel").stringValue();
            frame->addPar("tx_channel_ps") = node->par("txChannelPs").intValue();
            frame->setKind(0);
            pending.insert(frame);
            scheduleAt(ps(r.generation), frame);
        }
    }
    void handleMessage(cMessage *msg) override {
        auto frame = check_and_cast<CanDataFrame *>(msg);
        cModule *node = getParentModule();
        auto rec = recorder(node);
        if (frame->getKind() == 0) {
            rec->record("generated", node, frame);
            frame->setKind(1);
            // Every processing transition is a real FES event, even at zero delay.
            scheduleAt(simTime() + ps(node->par("txProcessingPs").intValue()), frame);
        }
        else {
            pending.erase(frame);
            auto buffer = check_and_cast<DirAdapterOutputBuffer *>(node->getSubmodule("bufferOut"));
            rec->record("ready", node, frame, buffer->queueWaiting());
            sendDirect(frame, node->getSubmodule("bufferOut")->gate("directIn"));
        }
    }
};
Define_Module(DirAdapterSource);

class DirAdapterSink : public cSimpleModule {
    std::set<cMessage *> pending;
    std::set<std::pair<bool, unsigned int>> accepted;
    bool all = false;
public:
    ~DirAdapterSink() override { for (auto msg : pending) cancelAndDelete(msg); }
protected:
    int numInitStages() const override { return 2; }
    void initialize(int stage) override {
        if (stage != 1) return;
        cModule *node = getParentModule();
        auto input = check_and_cast<FiCo4OMNeT::CanPortInput *>(node->getSubmodule("canNodePort")->getSubmodule("canPortInput"));
        for (unsigned int id : recorder(node)->canIds) input->registerIncomingDataFrame(id, gate("directIn"));
        std::string filter = node->par("rxFilter").stdstringValue();
        all = filter == "*";
        if (!all && filter != "none") {
            std::replace(filter.begin(), filter.end(), ',', ' ');
            std::istringstream tokens(filter);
            std::string token;
            while (tokens >> token) {
                const auto colon = token.find(':');
                std::string format = colon == std::string::npos ? "any" : token.substr(0, colon);
                std::string idText = colon == std::string::npos ? token : token.substr(colon + 1);
                if (format != "any" && format != "standard" && format != "extended")
                    throw cRuntimeError("Invalid rxFilter format");
                int64_t id;
                if (idText.size() > 2 && idText.substr(0, 2) == "0x") {
                    auto hex = idText.substr(2);
                    if (!std::all_of(hex.begin(), hex.end(), [](unsigned char c) { return std::isxdigit(c); }))
                        throw cRuntimeError("Invalid hexadecimal rxFilter ID");
                    try { id = std::stoll(hex, nullptr, 16); }
                    catch (const std::exception&) { throw cRuntimeError("rxFilter ID out of range"); }
                }
                else id = integer(idText, "rxFilter");
                if (id > (format == "standard" ? 0x7ff : 0x1fffffff)) throw cRuntimeError("rxFilter CAN ID out of range");
                if (format != "extended") accepted.insert({false, static_cast<unsigned int>(id)});
                if (format != "standard") accepted.insert({true, static_cast<unsigned int>(id)});
            }
        }
    }
    void handleMessage(cMessage *msg) override {
        auto frame = check_and_cast<CanDataFrame *>(msg);
        cModule *node = getParentModule();
        auto rec = recorder(node);
        if (!msg->isSelfMessage()) {
            rec->record("native_rx_complete", node, frame);
            frame->setKind(1);
            pending.insert(frame);
            scheduleAt(simTime() + ps(frame->par("tx_channel_ps").longValue()) + ps(node->par("rxChannelPs").intValue()), frame);
        }
        else if (frame->getKind() == 1) {
            rec->record("observed", node, frame);
            if (!all && accepted.count({frame->getExtendedId(), frame->getCanID()}) == 0) {
                rec->record("filtered", node, frame);
                pending.erase(frame);
                delete frame;
            }
            else {
                frame->setKind(2);
                scheduleAt(simTime() + ps(node->par("rxProcessingPs").intValue()), frame);
            }
        }
        else {
            rec->record("received", node, frame);
            pending.erase(frame);
            delete frame;
        }
    }
};
Define_Module(DirAdapterSink);
