// ElementIdentity.cpp — see ElementIdentity.h. SCHEMA §7.5.
#include "session/ElementIdentity.h"

#include <optional>
#include <string>

#include <TopoDS_Shape.hxx>

#include "elementmap/ElementMapPartition.h"

namespace onecad::session {

using nlohmann::json;
using protocol::Envelope;
namespace em = onecad::elementmap;

namespace {

std::string get_str(const json& o, const char* key, const std::string& dflt = "") {
    if (o.is_object() && o.contains(key) && o[key].is_string()) return o[key].get<std::string>();
    return dflt;
}

// The partition entry (if any) whose body_id==bodyId and topo_key==topo.
const em::PartitionEntry* entry_by_topokey(const em::ElementMapPartition& part,
                                           const std::string& body_id, const std::string& topo) {
    for (const em::PartitionEntry* e : part.entries_for_body(body_id)) {
        if (e->topo_key == topo) return e;
    }
    return nullptr;
}

// Resolve a pick's shape: topoKey (explicit), else anchor.worldPoint nearest.
TopoDS_Shape resolve_pick(const TopoDS_Shape& body, const json& pick, em::km::ElementKind& kind_out,
                          std::string& topo_out) {
    const std::string topo = get_str(pick, "topoKey");
    if (!topo.empty()) {
        TopoDS_Shape s = em::ElementMapPartition::shape_for_topokey(body, topo);
        if (!s.IsNull()) {
            topo_out = topo;
            switch (topo[0]) {
                case 'f': kind_out = em::km::ElementKind::Face; break;
                case 'e': kind_out = em::km::ElementKind::Edge; break;
                case 'v': kind_out = em::km::ElementKind::Vertex; break;
                default: kind_out = em::km::ElementKind::Unknown; break;
            }
            return s;
        }
    }
    // Anchor fallback (kind hint from pick.kind, default face).
    if (pick.contains("anchor") && pick["anchor"].is_object() &&
        pick["anchor"].contains("worldPoint") && pick["anchor"]["worldPoint"].is_array() &&
        pick["anchor"]["worldPoint"].size() >= 3) {
        const std::string kstr = get_str(pick, "kind", "face");
        em::km::ElementKind kind = em::ElementMapPartition::kind_from_name(kstr);
        if (kind == em::km::ElementKind::Unknown) kind = em::km::ElementKind::Face;
        const json& wp = pick["anchor"]["worldPoint"];
        TopoDS_Shape s = em::ElementMapPartition::nearest_subshape(
            body, kind, wp[0].get<double>(), wp[1].get<double>(), wp[2].get<double>());
        if (!s.IsNull()) {
            kind_out = kind;
            topo_out = em::ElementMapPartition::topokey_for_shape(body, s, kind);
            return s;
        }
    }
    return TopoDS_Shape();
}

}  // namespace

Envelope handle_acquire_element_ids(Session& session, const Envelope& req) {
    const json& args = req.args;
    const std::string body_id = get_str(args, "bodyId");
    const BodyStore bodies = session.bodies_copy();
    const em::ElementMapPartition part = session.partition_copy();

    const BodyRecord* rec = bodies.get(body_id);
    if (!rec) {
        return Envelope::error_response(
            req.id, protocol::ErrorInfo{"REF_UNRESOLVED", "AcquireElementIds: body not found: " + body_id,
                                        /*retriable=*/false});
    }

    json ids = json::array();
    if (args.contains("picks") && args["picks"].is_array()) {
        for (const json& pick : args["picks"]) {
            em::km::ElementKind kind = em::km::ElementKind::Unknown;
            std::string topo;
            TopoDS_Shape sub = resolve_pick(rec->geom, pick, kind, topo);
            if (sub.IsNull()) continue;  // unresolved pick → omitted (Rust re-picks)

            // Evidence the worker returns; RUST mints elementId. If the live
            // partition already holds an id for this binding, echo it (Invariant 1).
            const em::PartitionEntry* held = entry_by_topokey(part, body_id, topo);
            json entry = {
                {"topoKey", topo},
                {"kind", em::ElementMapPartition::kind_name(kind)},
                {"bodyId", body_id},
                {"elementId", held ? held->element_id : std::string("")},
                {"descriptor", em::ElementMapPartition::descriptor_to_json(
                                   em::ElementMapPartition::describe(sub))},
            };
            if (pick.contains("anchor")) entry["anchor"] = pick["anchor"];
            ids.push_back(std::move(entry));
        }
    }
    return Envelope::ok_response(req.id, json{{"ids", std::move(ids)}});
}

Envelope handle_query_element(Session& session, const Envelope& req) {
    const json& args = req.args;
    const BodyStore bodies = session.bodies_copy();
    const em::ElementMapPartition part = session.partition_copy();

    // By elementId (partition lookup).
    if (args.contains("elementId") && args["elementId"].is_string()) {
        const std::string eid = args["elementId"].get<std::string>();
        if (const em::PartitionEntry* e = part.find(eid)) {
            return Envelope::ok_response(
                req.id, json{{"elementId", eid},
                             {"topoKey", e->topo_key},
                             {"bodyId", e->body_id},
                             {"kind", em::ElementMapPartition::kind_name(e->kind)},
                             {"descriptor", em::ElementMapPartition::descriptor_to_json(e->descriptor)},
                             {"anchor", e->anchor.is_null() ? json::object() : e->anchor},
                             {"present", true}});
        }
        return Envelope::ok_response(req.id, json{{"elementId", eid}, {"present", false}});
    }

    // By {topoKey, bodyId} (shape lookup).
    const std::string topo = get_str(args, "topoKey");
    const std::string body_id = get_str(args, "bodyId");
    const BodyRecord* rec = bodies.get(body_id);
    if (rec && !topo.empty()) {
        TopoDS_Shape sub = em::ElementMapPartition::shape_for_topokey(rec->geom, topo);
        if (!sub.IsNull()) {
            const em::PartitionEntry* held = entry_by_topokey(part, body_id, topo);
            em::km::ElementKind kind = em::km::ElementKind::Unknown;
            switch (topo.empty() ? '?' : topo[0]) {
                case 'f': kind = em::km::ElementKind::Face; break;
                case 'e': kind = em::km::ElementKind::Edge; break;
                case 'v': kind = em::km::ElementKind::Vertex; break;
                default: break;
            }
            return Envelope::ok_response(
                req.id, json{{"elementId", held ? held->element_id : std::string("")},
                             {"topoKey", topo},
                             {"bodyId", body_id},
                             {"kind", em::ElementMapPartition::kind_name(kind)},
                             {"descriptor", em::ElementMapPartition::descriptor_to_json(
                                                em::ElementMapPartition::describe(sub))},
                             {"present", true}});
        }
    }
    return Envelope::ok_response(req.id, json{{"present", false}});
}

Envelope handle_resolve_refs(Session& session, const Envelope& req) {
    const json& args = req.args;
    const BodyStore bodies = session.bodies_copy();
    const em::ElementMapPartition part = session.partition_copy();

    json resolutions = json::array();
    if (args.contains("refs") && args["refs"].is_array()) {
        for (const json& ref : args["refs"]) {
            const std::string ref_id = get_str(ref, "refId");
            const json& pr = (ref.contains("primary") && ref["primary"].is_object()) ? ref["primary"]
                                                                                     : json::object();
            const std::string eid = get_str(pr, "elementId");
            // Already bound (unchanged) — descriptor echo, history-only (W-WP5).
            if (!eid.empty()) {
                if (const em::PartitionEntry* e = part.find(eid)) {
                    resolutions.push_back(json{{"refId", ref_id},
                                               {"outcome", "unchanged"},
                                               {"elementId", eid},
                                               {"topoKey", e->topo_key}});
                    continue;
                }
            }
            // Try to resolve against the referenced body's current shape.
            const std::string body_id = get_str(pr, "bodyId");
            const BodyRecord* rec = bodies.get(body_id);
            em::km::ElementKind kind = em::km::ElementKind::Unknown;
            std::string topo;
            TopoDS_Shape sub = rec ? resolve_pick(rec->geom, pr, kind, topo) : TopoDS_Shape();
            if (rec && sub.IsNull() && ref.contains("anchor")) {
                json pick = pr;
                pick["anchor"] = ref["anchor"];
                sub = resolve_pick(rec->geom, pick, kind, topo);
            }
            if (!sub.IsNull()) {
                // History/topoKey resolved uniquely → autoBind. Scoring is W-WP6;
                // W-WP5 reports a definitive unique resolution as full confidence.
                resolutions.push_back(json{{"refId", ref_id},
                                           {"outcome", "autoBind"},
                                           {"topoKey", topo},
                                           {"score", 1.0},
                                           {"margin", 1.0}});
            } else {
                resolutions.push_back(json{
                    {"refId", ref_id},
                    {"outcome", "needsRepair"},
                    {"needsRepair",
                     json{{"refId", ref_id},
                          {"elementId", eid},
                          {"ladderFailed", "descriptor"},
                          {"reason", "no-candidates"},
                          {"candidates", json::array()},
                          {"anchor", ref.contains("anchor") ? ref["anchor"] : json::object()},
                          {"uiLabel", "unresolved ref " + ref_id}}}});
            }
        }
    }
    return Envelope::ok_response(req.id, json{{"resolutions", std::move(resolutions)}});
}

}  // namespace onecad::session
