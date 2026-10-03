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
#include <deque>
#include <map>
#include <tuple>
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
    if (!std::getline(in, line)) throw cRuntimeError("Missing source TSV header: %s", path);
    if (!line.empty() && line.back() == '\r') line.pop_back();
    if (line != "generation_ps\trequest_id\tformat\tcan_id\tpayload_hex")
        throw cRuntimeError("Invalid source TSV header: %s", path);
    std::vector<Request> requests;
    std::set<std::string> ids;
    while (std::getline(in, line)) {
        if (!line.empty() && line.back() == '\r') line.pop_back();
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
    std::sort(requests.begin(), requests.end(), [](const Request& a, const Request& b) {
        return std::tie(a.generation, a.id) < std::tie(b.generation, b.id);
    });
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

namespace {
struct GatewayPort {
    std::string gateway, ingress;
    int64_t processing, capacity, hopLimit;
};
struct Route {
    GatewayPort config;
    std::string id, egress;
    bool extended;
    unsigned int minId, maxId;
};
struct RowContext {
    std::string gateway, ingress, egress, route, buffer, reason, child;
    int64_t rxUsed = -1;
    int64_t hops = -1;
};
struct RxHold {
    CanDataFrame *frame;
    GatewayPort config;
    size_t remaining;
};
struct ForwardTimer {
    std::string buffer;
    size_t route;
};
std::vector<std::string> columns(const std::string& line) {
    std::vector<std::string> fields;
    size_t from = 0;
    for (;;) {
        auto end = line.find('\t', from);
        fields.push_back(line.substr(from, end == std::string::npos ? end : end - from));
        if (end == std::string::npos) return fields;
        from = end + 1;
    }
}
std::string textPar(CanDataFrame *frame, const char *name) {
    return frame->hasPar(name) ? std::string(frame->par(name).stringValue()) : "";
}
void stringPar(CanDataFrame *frame, const char *name, const std::string& value) {
    if (frame->hasPar(name)) frame->par(name) = value.c_str();
    else frame->addPar(name) = value.c_str();
}
int64_t hopCount(CanDataFrame *frame) {
    return frame->hasPar("hops") ? frame->par("hops").longValue() : 0;
}
}

class DirAdapterOutputBuffer;
class DirAdapterRecorder : public cSimpleModule {
    std::ofstream out;
    cMessage *stop = nullptr;
    uint64_t sequence = 0;
    std::map<std::string, cModule *> nodes;
    std::map<std::string, GatewayPort> ports;
    std::vector<Route> routes;
    std::map<std::string, int64_t> rxUsed;
    std::map<std::string, RxHold> holds;
    std::map<cMessage *, ForwardTimer> forwards;
    std::set<CanDataFrame *> processing;
    std::map<std::string, std::deque<CanDataFrame *>> txWaiting;
    std::map<cMessage *, std::string> wakes;
    std::set<std::string> wakeScheduled;
    std::set<std::pair<std::string, std::string>> routed;
    void readRouting(const char *path) {
        if (!*path) return;
        std::ifstream in(path);
        if (!in) throw cRuntimeError("Cannot read routing TSV: %s", path);
        std::string line;
        if (!std::getline(in, line)) throw cRuntimeError("Missing routing TSV header: %s", path);
        if (!line.empty() && line.back() == '\r') line.pop_back();
        if (line != "kind\tgateway\tingress\troute_id\tegress\tformat\tid_min\tid_max\tprocessing_ps\trx_capacity\thop_limit")
            throw cRuntimeError("Invalid routing TSV header: %s", path);
        std::vector<std::vector<std::string>> rows;
        while (std::getline(in, line)) {
            if (!line.empty() && line.back() == '\r') line.pop_back();
            if (line.empty()) continue;
            auto fields = columns(line);
            if (fields.size() != 11) throw cRuntimeError("Routing TSV requires exactly 11 columns");
            rows.push_back(fields);
        }
        for (const auto& f : rows) {
            if (f[0] != "port") continue;
            GatewayPort config{f[1], f[2], integer(f[8], "processing_ps"), integer(f[9], "rx_capacity"), integer(f[10], "hop_limit")};
            if (config.gateway.empty() || !nodes.count(config.ingress) || config.hopLimit == 0 ||
                !ports.emplace(config.ingress, config).second)
                throw cRuntimeError("Invalid or duplicate Gateway port");
            for (size_t i = 3; i < 8; ++i)
                if (!f[i].empty()) throw cRuntimeError("Gateway port row contains route fields");
        }
        std::set<std::tuple<std::string, std::string, std::string>> branches;
        for (const auto& f : rows) {
            if (f[0] == "port") continue;
            if (f[0] != "route" || !ports.count(f[2]) || !ports.count(f[4]) || f[3].empty())
                throw cRuntimeError("Invalid Gateway route ports or kind");
            auto config = ports.at(f[2]);
            if (config.gateway != f[1] || ports.at(f[4]).gateway != f[1] || f[2] == f[4] ||
                config.processing != integer(f[8], "processing_ps") ||
                config.capacity != integer(f[9], "rx_capacity") ||
                config.hopLimit != integer(f[10], "hop_limit") ||
                !branches.emplace(f[2], f[3], f[4]).second)
                throw cRuntimeError("Inconsistent or duplicate Gateway route branch");
            if (f[5] != "standard" && f[5] != "extended") throw cRuntimeError("Invalid route format");
            auto lo = integer(f[6], "id_min"), hi = integer(f[7], "id_max");
            if (lo > hi || hi > (f[5] == "standard" ? 0x7ff : 0x1fffffff))
                throw cRuntimeError("Invalid route CAN ID interval");
            routes.push_back({config, f[3], f[4], f[5] == "extended", static_cast<unsigned int>(lo), static_cast<unsigned int>(hi)});
            // Copies retain a raw source's ID. Boundaries are also registered, without
            // allocating hundreds of millions of entries for an extended route range.
            canIds.insert(static_cast<unsigned int>(lo));
            canIds.insert(static_cast<unsigned int>(hi));
        }
        std::sort(routes.begin(), routes.end(), [](const Route& a, const Route& b) {
            return std::tie(a.config.gateway, a.config.ingress, a.extended, a.minId, a.id, a.egress) <
                   std::tie(b.config.gateway, b.config.ingress, b.extended, b.minId, b.id, b.egress);
        });
        std::map<std::string, GatewayPort> configs;
        for (const auto& entry : ports) {
            const auto& c = entry.second;
            auto prior = configs.emplace(c.gateway, c);
            if (!prior.second && std::tie(c.processing, c.capacity, c.hopLimit) !=
                std::tie(prior.first->second.processing, prior.first->second.capacity, prior.first->second.hopLimit))
                throw cRuntimeError("Inconsistent Gateway configuration between ports");
        }
        for (size_t i = 0; i < routes.size(); ++i) {
            for (size_t j = 0; j < i; ++j) {
                const auto& a = routes[i]; const auto& b = routes[j];
                if (a.config.ingress == b.config.ingress && a.extended == b.extended &&
                    a.minId <= b.maxId && b.minId <= a.maxId &&
                    (a.id != b.id || a.minId != b.minId || a.maxId != b.maxId))
                    throw cRuntimeError("Overlapping or inconsistent route intervals");
            }
        }
    }
    RowContext context(const std::string& key, const Route *route = nullptr) {
        const auto& hold = holds.at(key);
        RowContext c;
        c.gateway = hold.config.gateway; c.ingress = hold.config.ingress;
        c.buffer = key; c.rxUsed = rxUsed[c.ingress];
        if (route) { c.route = route->id; c.egress = route->egress; c.hops = hopCount(hold.frame) + 1; }
        return c;
    }
    void finishBranch(const std::string& key) {
        auto it = holds.find(key);
        if (it == holds.end() || !it->second.remaining) throw cRuntimeError("Gateway branch completed twice");
        --it->second.remaining;
        if (!it->second.remaining) releaseRx(key);
    }
    void releaseRx(const std::string& key) {
        auto it = holds.find(key);
        if (it == holds.end() || it->second.remaining) throw cRuntimeError("Invalid Gateway RX release");
        --rxUsed[it->second.config.ingress];
        auto c = context(key);
        record("rx_released", nodes.at(c.ingress), it->second.frame, -1, c);
        delete it->second.frame;
        holds.erase(it);
    }
    void childReady(CanDataFrame *frame);
    void drain(const std::string& egress);
public:
    int64_t horizon = 0;
    std::vector<std::vector<Request>> inputs;
    std::set<unsigned int> canIds;
    ~DirAdapterRecorder() override {
        cancelAndDelete(stop);
        for (auto& f : forwards) cancelAndDelete(f.first);
        for (auto& w : wakes) cancelAndDelete(w.first);
        for (auto f : processing) cancelAndDelete(f);
        for (auto& queue : txWaiting) for (auto f : queue.second) delete f;
        for (auto& hold : holds) delete hold.second.frame;
    }
    void record(const char *event, cModule *node, CanDataFrame *frame, int waiting = -1, const RowContext& c = {}) {
        if (simTime() >= ps(horizon)) return;
        out << event << ',' << simTime().inUnit(SIMTIME_PS) << ','
            << csv(node->par("nodeLabel").stdstringValue()) << ','
            << csv(frame->par("request_id").stringValue()) << ','
            << csv(frame->par("source").stringValue()) << ','
            << (frame->getExtendedId() ? "extended" : "standard") << ',' << frame->getCanID() << ','
            << csv(payloadHex(*frame)) << ',' << frame->getBitLength() << ',';
        if (waiting >= 0) out << waiting;
        const auto origin = textPar(frame, "origin_request_id");
        out << ',' << sequence++ << ',' << csv(origin.empty() ? textPar(frame, "request_id") : origin) << ','
            << csv(c.gateway.empty() ? textPar(frame, "parent_request_id") : textPar(frame, "request_id")) << ','
            << (c.hops < 0 ? hopCount(frame) : c.hops) << ',' << csv(c.gateway) << ',' << csv(c.ingress) << ','
            << csv(c.egress) << ',' << csv(c.route) << ',' << csv(c.buffer) << ',' << csv(c.reason) << ',';
        if (c.rxUsed >= 0) out << c.rxUsed;
        out << ',' << csv(c.child) << '\n';
        out.flush();
        if (!out) throw cRuntimeError("Writing adapter CSV failed");
    }
    bool gatewayReceive(cModule *node, CanDataFrame *frame) {
        Enter_Method_Silent();
        const auto ingress = node->par("nodeLabel").stdstringValue();
        if (!ports.count(ingress)) return false;
        const auto config = ports.at(ingress);
        const auto parent = textPar(frame, "request_id");
        if (!routed.emplace(parent, ingress).second) throw cRuntimeError("Duplicate Gateway reception");
        const auto key = "rx:" + parent + "/" + config.gateway + "/" + ingress;
        RowContext c; c.gateway = config.gateway; c.ingress = ingress; c.buffer = key; c.rxUsed = rxUsed[ingress];
        if (rxUsed[ingress] >= config.capacity) {
            c.reason = "rx_queue_full";
            record("rx_dropped", node, frame, -1, c);
            take(frame); delete frame;
            return true;
        }
        take(frame);
        std::vector<size_t> matching;
        for (size_t i = 0; i < routes.size(); ++i) {
            const auto& r = routes[i];
            if (r.config.ingress == ingress && r.extended == frame->getExtendedId() &&
                r.minId <= frame->getCanID() && frame->getCanID() <= r.maxId) matching.push_back(i);
        }
        holds.emplace(key, RxHold{frame, config, matching.size()});
        ++rxUsed[ingress]; c.rxUsed = rxUsed[ingress];
        record("rx_admitted", node, frame, -1, c);
        if (matching.empty()) {
            c.reason = "no_route";
            record("route_filtered", node, frame, -1, c);
            releaseRx(key);
        }
        else for (auto index : matching) {
            const auto& route = routes[index];
            record("forward_pending", nodes.at(route.egress), frame, -1, context(key, &route));
            auto timer = new cMessage("gateway-processing");
            forwards.emplace(timer, ForwardTimer{key, index});
            scheduleAt(simTime() + ps(config.processing), timer);
        }
        return true;
    }
    void slotFreed(cModule *node) {
        Enter_Method_Silent();
        const auto label = node->par("nodeLabel").stdstringValue();
        if (!txWaiting[label].empty() && wakeScheduled.insert(label).second) {
            auto wake = new cMessage("gateway-tx-slot-freed");
            wakes.emplace(wake, label);
            // Enqueue only after the native bus finishes notifying every SOF participant.
            scheduleAt(simTime(), wake);
        }
    }
protected:
    void initialize() override {
        if (SimTime::getScaleExp() != -12) throw cRuntimeError("Adapter requires simtime-resolution=ps");
        cModule *network = getParentModule();
        horizon = network->par("horizonPs").intValue();
        int count = network->par("nodeCount").intValue();
        if (horizon < 0 || count < 2 || (network->hasPar("bitrate") && network->par("bitrate").intValue() <= 0))
            throw cRuntimeError("Invalid horizonPs, nodeCount or bitrate");
        out.open(network->par("outputFile").stringValue());
        if (!out) throw cRuntimeError("Cannot open adapter outputFile");
        out << "event,time_ps,node,request_id,source,format,can_id,payload_hex,native_bits,queue_waiting,sequence,origin_request_id,parent_request_id,hops,gateway,ingress,egress,route_id,buffer_id,reason,rx_used,child_request_id\n";
        out.flush();
        for (int i = 0; i < count; ++i) {
            cModule *node = network->getSubmodule("node", i);
            if (!node || !nodes.emplace(node->par("nodeLabel").stdstringValue(), node).second)
                throw cRuntimeError("Missing node or duplicate nodeLabel");
            for (const char *param : {"queueCapacity", "txProcessingPs", "rxProcessingPs", "txChannelPs", "rxChannelPs"})
                if (node->par(param).intValue() < 0) throw cRuntimeError("Negative node parameter %s", param);
            inputs.push_back(readRequests(node->par("sourceFile").stringValue()));
            for (const auto& r : inputs.back()) canIds.insert(r.canId);
        }
        if (network->hasPar("routingFile")) readRouting(network->par("routingFile").stringValue());
        stop = new cMessage("exclusive-horizon");
        stop->setSchedulingPriority(std::numeric_limits<short>::min());
        scheduleAt(ps(horizon), stop);
    }
    void handleMessage(cMessage *msg) override {
        if (msg == stop) { endSimulation(); return; }
        auto w = wakes.find(msg);
        if (w != wakes.end()) {
            auto label = w->second; wakes.erase(w); wakeScheduled.erase(label);
            delete msg; drain(label); return;
        }
        auto f = forwards.find(msg);
        if (f != forwards.end()) {
            auto timer = f->second; forwards.erase(f); delete msg;
            auto& hold = holds.at(timer.buffer);
            const auto& route = routes.at(timer.route);
            auto c = context(timer.buffer, &route);
            if (hopCount(hold.frame) + 1 > hold.config.hopLimit) {
                c.reason = "dropped_hop_limit";
                record("forward_dropped", nodes.at(route.egress), hold.frame, -1, c);
                finishBranch(timer.buffer);
            }
            else {
                auto child = hold.frame->dup();
                const auto childId = "gw:" + textPar(hold.frame, "request_id") + "/" + hold.config.gateway + "/" + route.id + "/" + route.egress;
                child->setName(childId.c_str()); child->par("request_id") = childId.c_str();
                child->par("parent_request_id") = textPar(hold.frame, "request_id").c_str();
                child->par("source") = route.egress.c_str(); child->par("hops") = hopCount(hold.frame) + 1;
                child->par("tx_channel_ps") = nodes.at(route.egress)->par("txChannelPs").intValue();
                stringPar(child, "rx_buffer_id", timer.buffer);
                stringPar(child, "gateway", hold.config.gateway);
                stringPar(child, "ingress", hold.config.ingress);
                stringPar(child, "route_id", route.id);
                c.child = childId;
                record("forward_submitted", nodes.at(route.egress), hold.frame, -1, c);
                record("generated", nodes.at(route.egress), child);
                processing.insert(child);
                scheduleAt(simTime() + ps(nodes.at(route.egress)->par("txProcessingPs").intValue()), child);
            }
            return;
        }
        auto child = check_and_cast<CanDataFrame *>(msg);
        if (!processing.erase(child)) throw cRuntimeError("Unknown Gateway processing timer");
        childReady(child);
    }
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
        Enter_Method_Silent();
        take(msg);
        auto frame = check_and_cast<CanDataFrame *>(msg);
        cModule *node = getParentModule();
        int waiting = queueWaiting();
        if (waiting >= node->par("queueCapacity").intValue()) {
            RowContext c; c.reason = "queue_full";
            recorder(node)->record("dropped", node, frame, waiting, c);
            delete frame;
            return;
        }
        FiCo4OMNeT::CanOutputBuffer::putFrame(msg);
        recorder(node)->record("enqueued", node, frame, queueWaiting());
    }
    void receiveSendingPermission(unsigned int id, bool extended, bool rtr) override {
        FiCo4OMNeT::CanOutputBuffer::receiveSendingPermission(id, extended, rtr);
        recorder(getParentModule())->record("sof", getParentModule(), currentFrame, frames.size() - 1);
        recorder(getParentModule())->slotFreed(getParentModule());
    }
    void sendingCompleted() override {
        recorder(getParentModule())->record("native_complete", getParentModule(), currentFrame, frames.size() - 1);
        FiCo4OMNeT::CanOutputBuffer::sendingCompleted();
    }
};
Define_Module(DirAdapterOutputBuffer);

void DirAdapterRecorder::childReady(CanDataFrame *frame) {
    const auto egress = textPar(frame, "source");
    const auto key = textPar(frame, "rx_buffer_id");
    auto node = nodes.at(egress);
    auto buffer = check_and_cast<DirAdapterOutputBuffer *>(node->getSubmodule("bufferOut"));
    record("ready", node, frame, buffer->queueWaiting());
    if (node->par("queueCapacity").intValue() == 0) {
        RowContext c; c.reason = "queue_full";
        record("dropped", node, frame, buffer->queueWaiting(), c);
        delete frame; finishBranch(key);
    }
    else if (buffer->queueWaiting() >= node->par("queueCapacity").intValue() || !txWaiting[egress].empty()) {
        txWaiting[egress].push_back(frame);
        record("waiting_tx", node, frame, buffer->queueWaiting());
    }
    else {
        buffer->putFrame(frame);
        finishBranch(key);
    }
}

void DirAdapterRecorder::drain(const std::string& egress) {
    auto node = nodes.at(egress);
    auto buffer = check_and_cast<DirAdapterOutputBuffer *>(node->getSubmodule("bufferOut"));
    auto& queue = txWaiting[egress];
    while (!queue.empty() && buffer->queueWaiting() < node->par("queueCapacity").intValue()) {
        auto child = queue.front(); queue.pop_front();
        auto key = textPar(child, "rx_buffer_id");
        buffer->putFrame(child);
        finishBranch(key);
    }
}

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
            frame->addPar("origin_request_id") = r.id.c_str();
            frame->addPar("parent_request_id") = "";
            frame->addPar("hops") = 0L;
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
            if (!rec->gatewayReceive(node, frame)) delete frame;
        }
    }
};
Define_Module(DirAdapterSink);
