// WireSketch.cpp — see WireSketch.h. Wire (SCHEMA §7.3/§7.4) -> ported Sketch.
#include "sketch/WireSketch.h"

#include <algorithm>
#include <cctype>
#include <cmath>

#include "sketch/SketchArc.h"
#include "sketch/SketchCircle.h"
#include "sketch/SketchLine.h"
#include "sketch/SketchPoint.h"
#include "sketch/constraints/Constraints.h"

namespace onecad::wire {

namespace {

namespace cs = onecad::core::sketch::constraints;
using nlohmann::json;

std::string lower(std::string s) {
    std::transform(s.begin(), s.end(), s.begin(),
                   [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    return s;
}

// Read a [x,y] array; returns false if not a 2-number array.
bool read_vec2(const json& j, double& x, double& y) {
    if (!j.is_array() || j.size() < 2 || !j[0].is_number() || !j[1].is_number()) return false;
    x = j[0].get<double>();
    y = j[1].get<double>();
    return std::isfinite(x) && std::isfinite(y);
}

bool read_vec3(const json& j, sk::Vec3d& v) {
    if (!j.is_array() || j.size() < 3) return false;
    for (int i = 0; i < 3; ++i) {
        if (!j[i].is_number()) return false;
    }
    v = {j[0].get<double>(), j[1].get<double>(), j[2].get<double>()};
    return true;
}

// A dimensional value may be a bare number OR a {value, expr?} object (SCHEMA §7.3).
bool read_scalar(const json& field, double& out) {
    if (field.is_number()) {
        out = field.get<double>();
        return std::isfinite(out);
    }
    if (field.is_object() && field.contains("value") && field["value"].is_number()) {
        out = field["value"].get<double>();
        return std::isfinite(out);
    }
    return false;
}

// First present scalar among the given keys.
bool scalar_field(const json& c, std::initializer_list<const char*> keys, double& out) {
    for (const char* k : keys) {
        if (c.contains(k) && read_scalar(c[k], out)) return true;
    }
    return false;
}

// Position of a Point entity from any of the accepted coordinate keys.
bool read_point_pos(const json& e, double& x, double& y) {
    for (const char* k : {"at", "p", "pos", "position", "xy", "p0"}) {
        if (e.contains(k) && read_vec2(e[k], x, y)) return true;
    }
    return false;
}

}  // namespace

// --- WireIndex --------------------------------------------------------------

sk::EntityID WireIndex::resolve_point(const std::string& entity_id,
                                      const std::string& role) const {
    if (role.empty()) {
        auto it = handle_to_point.find(entity_id);
        if (it != handle_to_point.end()) return it->second;
        return {};
    }
    const std::string key = entity_id + "." + lower(role);
    auto it = handle_to_point.find(key);
    if (it != handle_to_point.end()) return it->second;
    // Fall back: a Point entity addressed with a spurious role.
    auto pit = handle_to_point.find(entity_id);
    if (pit != handle_to_point.end()) return pit->second;
    return {};
}

std::string WireIndex::handle_for(const sk::EntityID& internal_point) const {
    auto it = point_to_handle.find(internal_point);
    return it != point_to_handle.end() ? it->second : std::string{};
}

// --- plane ------------------------------------------------------------------

namespace {

sk::SketchPlane parse_plane(const json& args, std::string& err) {
    if (!args.contains("plane") || !args["plane"].is_object()) {
        return sk::SketchPlane::XY();  // default
    }
    const json& p = args["plane"];
    const std::string kind = p.value("kind", std::string{"XY"});
    if (kind == "XY") return sk::SketchPlane::XY();
    if (kind == "XZ") return sk::SketchPlane::XZ();
    if (kind == "YZ") return sk::SketchPlane::YZ();
    if (kind == "custom") {
        sk::SketchPlane plane = sk::SketchPlane::XY();
        if (p.contains("origin")) read_vec3(p["origin"], plane.origin);
        if (p.contains("xAxis")) read_vec3(p["xAxis"], plane.xAxis);
        if (p.contains("yAxis")) read_vec3(p["yAxis"], plane.yAxis);
        if (p.contains("normal")) read_vec3(p["normal"], plane.normal);
        return plane;
    }
    err = "unknown plane.kind '" + kind + "'";
    return sk::SketchPlane::XY();
}

}  // namespace

// --- translate --------------------------------------------------------------

TranslateResult translate(const json& args) {
    TranslateResult result;
    std::string plane_err;
    sk::SketchPlane plane = parse_plane(args, plane_err);
    if (!plane_err.empty()) {
        result.error = plane_err;
        return result;
    }

    auto sketch = std::make_unique<sk::Sketch>(plane);
    WireIndex idx;

    const json entities = args.value("entities", json::array());
    const json constraints = args.value("constraints", json::array());
    if (!entities.is_array()) {
        result.error = "entities must be an array";
        return result;
    }

    auto reg_handle = [&](const std::string& handle, const sk::EntityID& pid, bool primary) {
        idx.handle_to_point[handle] = pid;
        if (primary && idx.point_to_handle.find(pid) == idx.point_to_handle.end()) {
            idx.point_to_handle[pid] = handle;
        }
    };

    // Pass 1: Point entities (lines may reference them by id).
    for (const json& e : entities) {
        if (!e.is_object()) {
            result.error = "entity must be an object";
            return result;
        }
        const std::string type = e.value("type", std::string{});
        if (type != "Point") continue;
        const std::string id = e.value("id", std::string{});
        if (id.empty()) {
            result.error = "point entity missing id";
            return result;
        }
        double x = 0, y = 0;
        if (!read_point_pos(e, x, y)) {
            result.error = "point '" + id + "' missing position";
            return result;
        }
        const sk::EntityID pid = sketch->addPoint(x, y, /*construction=*/false);
        idx.wire_to_internal[id] = pid;
        reg_handle(id, pid, /*primary=*/true);
    }

    // Pass 2: Line / Arc / Circle entities.
    for (const json& e : entities) {
        const std::string type = e.value("type", std::string{});
        if (type == "Point") continue;
        const std::string id = e.value("id", std::string{});
        if (id.empty()) {
            result.error = "entity missing id";
            return result;
        }

        if (type == "Line") {
            sk::EntityID sp, ep;
            if (e.contains("p0Ref") && e.contains("p1Ref")) {
                const auto s = idx.wire_to_internal.find(e["p0Ref"].get<std::string>());
                const auto t = idx.wire_to_internal.find(e["p1Ref"].get<std::string>());
                if (s == idx.wire_to_internal.end() || t == idx.wire_to_internal.end()) {
                    result.error = "line '" + id + "' references unknown point";
                    return result;
                }
                sp = s->second;
                ep = t->second;
            } else {
                double x0, y0, x1, y1;
                if (!e.contains("p0") || !e.contains("p1") ||
                    !read_vec2(e["p0"], x0, y0) || !read_vec2(e["p1"], x1, y1)) {
                    result.error = "line '" + id + "' missing p0/p1";
                    return result;
                }
                sp = sketch->addPoint(x0, y0, false);
                ep = sketch->addPoint(x1, y1, false);
                reg_handle(id + ".p0", sp, true);
                reg_handle(id + ".p1", ep, true);
            }
            // Aliases valid for both inline and ref lines.
            reg_handle(id + ".p0", sp, false);
            reg_handle(id + ".start", sp, false);
            reg_handle(id + ".p1", ep, false);
            reg_handle(id + ".end", ep, false);
            const sk::EntityID lid = sketch->addLine(sp, ep, false);
            if (lid.empty()) {
                result.error = "line '" + id + "' could not be created";
                return result;
            }
            idx.wire_to_internal[id] = lid;
            idx.internal_edge_to_wire[lid] = id;
        } else if (type == "Circle") {
            double cx, cy, radius;
            if (!e.contains("center") || !read_vec2(e["center"], cx, cy) ||
                !scalar_field(e, {"radius"}, radius)) {
                result.error = "circle '" + id + "' missing center/radius";
                return result;
            }
            const sk::EntityID cp = sketch->addPoint(cx, cy, false);
            reg_handle(id + ".center", cp, true);
            const sk::EntityID cid = sketch->addCircle(cp, radius, false);
            if (cid.empty()) {
                result.error = "circle '" + id + "' could not be created";
                return result;
            }
            idx.wire_to_internal[id] = cid;
            idx.internal_edge_to_wire[cid] = id;
        } else if (type == "Arc") {
            double cx, cy, radius;
            if (!e.contains("center") || !read_vec2(e["center"], cx, cy) ||
                !scalar_field(e, {"radius"}, radius)) {
                result.error = "arc '" + id + "' missing center/radius";
                return result;
            }
            double start_angle = 0.0, end_angle = 0.0;
            double sx, sy, ex, ey;
            if (e.contains("start") && e.contains("end") && read_vec2(e["start"], sx, sy) &&
                read_vec2(e["end"], ex, ey)) {
                start_angle = std::atan2(sy - cy, sx - cx);
                end_angle = std::atan2(ey - cy, ex - cx);
            } else {
                scalar_field(e, {"startAngle"}, start_angle);
                scalar_field(e, {"endAngle"}, end_angle);
            }
            const sk::EntityID cp = sketch->addPoint(cx, cy, false);
            reg_handle(id + ".center", cp, true);
            const sk::EntityID aid = sketch->addArc(cp, radius, start_angle, end_angle, false);
            if (aid.empty()) {
                result.error = "arc '" + id + "' could not be created";
                return result;
            }
            idx.wire_to_internal[id] = aid;
            idx.internal_edge_to_wire[aid] = id;
        } else {
            result.error = "unsupported entity type '" + type + "'";
            return result;
        }
    }

    // Pass 3: constraints.
    if (!constraints.is_null() && !constraints.is_array()) {
        result.error = "constraints must be an array";
        return result;
    }
    for (const json& c : constraints) {
        const std::string type = c.value("type", std::string{});
        const json ents = c.value("entities", json::array());
        const json poss = c.value("positions", json::array());
        auto ent = [&](std::size_t i) -> std::string {
            return (i < ents.size() && ents[i].is_string()) ? ents[i].get<std::string>()
                                                             : std::string{};
        };
        auto pos = [&](std::size_t i) -> std::string {
            return (i < poss.size() && poss[i].is_string()) ? lower(poss[i].get<std::string>())
                                                            : std::string{};
        };
        auto internal = [&](const std::string& wid) -> sk::EntityID {
            auto it = idx.wire_to_internal.find(wid);
            return it != idx.wire_to_internal.end() ? it->second : std::string{};
        };
        auto is_point_entity = [&](const std::string& wid) -> bool {
            const sk::EntityID iid = internal(wid);
            const sk::SketchEntity* se = iid.empty() ? nullptr : sketch->getEntity(iid);
            return se && se->type() == sk::EntityType::Point;
        };
        // Distance/HDist/VDist operand: a point (via handle) or a line entity id.
        auto operand = [&](std::size_t i) -> sk::EntityID {
            if (!pos(i).empty()) return idx.resolve_point(ent(i), pos(i));
            if (is_point_entity(ent(i))) return internal(ent(i));
            return internal(ent(i));  // line
        };

        sk::ConstraintID added;
        std::string fail;
        double value = 0.0;

        if (type == "Coincident") {
            const sk::EntityID a = idx.resolve_point(ent(0), pos(0));
            const sk::EntityID b = idx.resolve_point(ent(1), pos(1));
            if (a.empty() || b.empty()) fail = "Coincident: unresolved point handle";
            else added = sketch->addCoincident(a, b);
        } else if (type == "Horizontal") {
            added = sketch->addHorizontal(internal(ent(0)));
        } else if (type == "Vertical") {
            added = sketch->addVertical(internal(ent(0)));
        } else if (type == "Fixed") {
            const sk::EntityID p = idx.resolve_point(ent(0), pos(0));
            if (p.empty()) fail = "Fixed: unresolved point handle";
            else added = sketch->addFixed(p);
        } else if (type == "Midpoint") {
            const sk::EntityID p = idx.resolve_point(ent(0), pos(0));
            const sk::EntityID l = internal(ent(1));
            if (p.empty() || l.empty()) fail = "Midpoint: unresolved refs";
            else added = sketch->addConstraint(std::make_unique<cs::MidpointConstraint>(p, l));
        } else if (type == "OnCurve") {
            const sk::EntityID p = idx.resolve_point(ent(0), pos(0));
            const sk::EntityID cv = internal(ent(1));
            sk::CurvePosition cp = sk::CurvePosition::Arbitrary;
            const std::string role = pos(1);
            if (role == "start") cp = sk::CurvePosition::Start;
            else if (role == "end") cp = sk::CurvePosition::End;
            if (p.empty() || cv.empty()) fail = "OnCurve: unresolved refs";
            else added = sketch->addPointOnCurve(p, cv, cp);
        } else if (type == "Parallel") {
            added = sketch->addParallel(internal(ent(0)), internal(ent(1)));
        } else if (type == "Perpendicular") {
            added = sketch->addPerpendicular(internal(ent(0)), internal(ent(1)));
        } else if (type == "Tangent") {
            const sk::EntityID a = internal(ent(0)), b = internal(ent(1));
            if (a.empty() || b.empty()) fail = "Tangent: unresolved refs";
            else added = sketch->addConstraint(std::make_unique<cs::TangentConstraint>(a, b));
        } else if (type == "Concentric") {
            added = sketch->addConcentric(internal(ent(0)), internal(ent(1)));
        } else if (type == "Equal") {
            const sk::EntityID a = internal(ent(0)), b = internal(ent(1));
            if (a.empty() || b.empty()) fail = "Equal: unresolved refs";
            else added = sketch->addConstraint(std::make_unique<cs::EqualConstraint>(a, b));
        } else if (type == "Distance") {
            if (!scalar_field(c, {"value", "distance"}, value)) fail = "Distance: missing value";
            else if (ents.size() == 1 && !is_point_entity(ent(0))) {
                // Single line => length between its endpoints.
                const sk::EntityID s = idx.resolve_point(ent(0), "p0");
                const sk::EntityID t = idx.resolve_point(ent(0), "p1");
                if (s.empty() || t.empty()) fail = "Distance: line has no endpoints";
                else added = sketch->addDistance(s, t, value);
            } else {
                const sk::EntityID a = operand(0), b = operand(1);
                if (a.empty() || b.empty()) fail = "Distance: unresolved operands";
                else added = sketch->addDistance(a, b, value);
            }
        } else if (type == "HorizontalDistance") {
            const sk::EntityID a = idx.resolve_point(ent(0), pos(0));
            const sk::EntityID b = idx.resolve_point(ent(1), pos(1));
            if (!scalar_field(c, {"value", "distance"}, value)) fail = "HorizontalDistance: missing value";
            else if (a.empty() || b.empty()) fail = "HorizontalDistance: unresolved points";
            else added = sketch->addHorizontalDistance(a, b, value);
        } else if (type == "VerticalDistance") {
            const sk::EntityID a = idx.resolve_point(ent(0), pos(0));
            const sk::EntityID b = idx.resolve_point(ent(1), pos(1));
            if (!scalar_field(c, {"value", "distance"}, value)) fail = "VerticalDistance: missing value";
            else if (a.empty() || b.empty()) fail = "VerticalDistance: unresolved points";
            else added = sketch->addVerticalDistance(a, b, value);
        } else if (type == "Angle") {
            if (!scalar_field(c, {"value", "angleDeg", "angle"}, value)) fail = "Angle: missing value";
            else added = sketch->addAngle(internal(ent(0)), internal(ent(1)), value);
        } else if (type == "Radius") {
            if (!scalar_field(c, {"value", "radius"}, value)) fail = "Radius: missing value";
            else added = sketch->addRadius(internal(ent(0)), value);
        } else if (type == "Diameter") {
            if (!scalar_field(c, {"value", "diameter"}, value)) fail = "Diameter: missing value";
            else added = sketch->addDiameter(internal(ent(0)), value);
        } else if (type == "Symmetric") {
            const sk::EntityID a = idx.resolve_point(ent(0), pos(0));
            const sk::EntityID b = idx.resolve_point(ent(1), pos(1));
            const sk::EntityID axis = internal(ent(2));
            if (a.empty() || b.empty() || axis.empty()) fail = "Symmetric: unresolved refs";
            else added = sketch->addSymmetric(a, b, axis);
        } else {
            fail = "unknown constraint type '" + type + "'";
        }

        if (!fail.empty()) {
            result.error = fail;
            return result;
        }
        if (added.empty()) {
            result.error = "constraint '" + c.value("id", type) + "' (" + type + ") rejected";
            return result;
        }
        const std::string wire_cid = c.value("id", std::string{});
        if (!wire_cid.empty()) idx.internal_constraint_to_wire[added] = wire_cid;
    }

    result.sketch = std::move(sketch);
    result.index = std::move(idx);
    result.ok = true;
    return result;
}

// --- write-back -------------------------------------------------------------

void apply_solved_positions(json& args, const sk::Sketch& sketch, const WireIndex& index) {
    if (!args.contains("entities") || !args["entities"].is_array()) return;
    auto point_pos = [&](const sk::EntityID& pid, double& x, double& y) -> bool {
        const auto* p = sketch.getEntityAs<sk::SketchPoint>(pid);
        if (!p) return false;
        x = p->position().X();
        y = p->position().Y();
        return true;
    };
    for (json& e : args["entities"]) {
        const std::string type = e.value("type", std::string{});
        const std::string id = e.value("id", std::string{});
        if (id.empty()) continue;
        double x, y;
        if (type == "Point") {
            const auto it = index.wire_to_internal.find(id);
            if (it != index.wire_to_internal.end() && point_pos(it->second, x, y)) {
                e["at"] = json::array({x, y});
            }
        } else if (type == "Line") {
            if (e.contains("p0Ref")) continue;  // endpoints are Point entities, updated above
            const sk::EntityID s = index.resolve_point(id, "p0");
            const sk::EntityID t = index.resolve_point(id, "p1");
            if (!s.empty() && point_pos(s, x, y)) e["p0"] = json::array({x, y});
            if (!t.empty() && point_pos(t, x, y)) e["p1"] = json::array({x, y});
        } else if (type == "Circle") {
            const sk::EntityID cp = index.resolve_point(id, "center");
            if (!cp.empty() && point_pos(cp, x, y)) e["center"] = json::array({x, y});
            const auto it = index.wire_to_internal.find(id);
            if (it != index.wire_to_internal.end()) {
                if (const auto* c = sketch.getEntityAs<sk::SketchCircle>(it->second)) {
                    e["radius"] = c->radius();
                }
            }
        } else if (type == "Arc") {
            const sk::EntityID cp = index.resolve_point(id, "center");
            double cx = 0, cy = 0;
            const bool have_center = !cp.empty() && point_pos(cp, cx, cy);
            if (have_center) e["center"] = json::array({cx, cy});
            const auto it = index.wire_to_internal.find(id);
            if (it != index.wire_to_internal.end()) {
                if (const auto* a = sketch.getEntityAs<sk::SketchArc>(it->second)) {
                    e["radius"] = a->radius();
                    if (have_center && e.contains("start") && e.contains("end")) {
                        const gp_Pnt2d c(cx, cy);
                        const gp_Pnt2d sp = a->startPoint(c);
                        const gp_Pnt2d ep = a->endPoint(c);
                        e["start"] = json::array({sp.X(), sp.Y()});
                        e["end"] = json::array({ep.X(), ep.Y()});
                    }
                }
            }
        }
    }
}

}  // namespace onecad::wire
