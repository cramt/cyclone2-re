/*
 * nextpnr himbaechel uarch for the Altera Cyclone II EP2C35F672C6.
 *
 * The chip database comes from `cram chipdb` (tools/cram/src/chipdb.rs, which
 * documents the tile and wire naming this file relies on) via gen_chipdb.py.
 *
 * Cells (from synth/cyclone2.ys):
 *   LCELL_COMB  LUT4 + carry: DATAA..DATAD, CIN -> COMBOUT, COUT; LUT_MASK.
 *               With CIN connected the LUT's C input is the carry, as in
 *               Quartus' sum_lutc_input = "cin".
 *   LCELL_FF    register fed by its own LE's LUT: DATAIN, CLK -> REGOUT.
 *   CY2_IBUF / CY2_OBUF  pins, placed by `--vopt qsf=<file>` location
 *               assignments (`set_location_assignment PIN_x -to port`).
 *   CLKCTRL     global clock control, inserted here on every clock net.
 */

#include <fstream>
#include <queue>
#include <regex>

#include "himbaechel_api.h"
#include "log.h"
#include "nextpnr.h"
#include "util.h"

#include "himbaechel_helpers.h"

#define GEN_INIT_CONSTIDS
#define HIMBAECHEL_CONSTIDS "uarch/cyclone2/constids.inc"
#define HIMBAECHEL_GFXIDS "uarch/cyclone2/gfxids.inc"
#define HIMBAECHEL_UARCH cyclone2
#include "himbaechel_constids.h"
#include "himbaechel_gfxids.h"

NEXTPNR_NAMESPACE_BEGIN

namespace {

const IdString lut_inputs[4] = {id_DATAA, id_DATAB, id_DATAC, id_DATAD};

struct Cyclone2Impl : HimbaechelAPI
{
    ~Cyclone2Impl() {};

    po::options_description getUArchOptions() override
    {
        po::options_description specific("Cyclone II options");
        specific.add_options()("qsf", po::value<std::string>(),
                               "Quartus-style settings file with `set_location_assignment PIN_x -to port` lines");
        return specific;
    }

    void init_database(Arch *arch) override
    {
        init_uarch_constids(arch);
        arch->load_chipdb("cyclone2/chipdb-EP2C35F672C6.bin");
        arch->set_speed_grade("C6");
    }

    void init(Context *ctx) override
    {
        h.init(ctx);
        HimbaechelAPI::init(ctx);
    }

    std::string getDefaultRouter() const override { return "router2"; }

    // --- packing ---

    void pack() override
    {
        pack_io();
        pack_constants();
        pack_lut_ff();
        pack_carry();
        pack_clocks();
    }

    dict<std::string, std::string> read_pin_assignments()
    {
        dict<std::string, std::string> port2pin;
        if (!ctx->args.options.count("qsf"))
            return port2pin;
        std::string path = ctx->args.options.at("qsf").as<std::string>();
        std::ifstream f(path);
        if (!f)
            log_error("cannot open %s\n", path.c_str());
        std::regex re(R"re(^\s*set_location_assignment\s+PIN_(\w+)\s+-to\s+"?([^"\s]+)"?)re");
        std::string line;
        while (std::getline(f, line)) {
            std::smatch m;
            if (std::regex_search(line, m, re))
                port2pin[m[2]] = m[1];
        }
        return port2pin;
    }

    void pack_io()
    {
        // Remember which top-level port each pin buffer serves, then drop
        // nextpnr's own port cells (yosys already inserted ours).
        for (auto &cell : ctx->cells) {
            CellInfo *ci = cell.second.get();
            if (ci->type == ctx->id("$nextpnr_ibuf")) {
                NetInfo *o = ci->getPort(ctx->id("O"));
                if (o)
                    for (auto &usr : o->users)
                        if (usr.cell->type == id_CY2_IBUF)
                            usr.cell->setAttr(id_PORT, ci->name.str(ctx));
            } else if (ci->type == ctx->id("$nextpnr_obuf")) {
                NetInfo *i = ci->getPort(ctx->id("I"));
                if (i && i->driver.cell && i->driver.cell->type == id_CY2_OBUF)
                    i->driver.cell->setAttr(id_PORT, ci->name.str(ctx));
            } else if (ci->type == ctx->id("$nextpnr_iobuf")) {
                log_error("bidirectional port %s is not supported yet\n", ctx->nameOf(ci));
            }
        }
        h.remove_nextpnr_iobs({CellTypePort(id_CY2_IBUF, id_PAD), CellTypePort(id_CY2_OBUF, id_PAD)});

        dict<std::string, BelId> pin2bel;
        for (auto &pad : ctx->chip_info->packages[0].pads) {
            IdStringList name = IdStringList::concat(IdString(pad.tile), IdString(pad.bel));
            pin2bel[IdString(pad.package_pin).str(ctx)] = ctx->getBelByName(name);
        }
        auto port2pin = read_pin_assignments();
        for (auto &cell : ctx->cells) {
            CellInfo *ci = cell.second.get();
            if (!ci->type.in(id_CY2_IBUF, id_CY2_OBUF))
                continue;
            if (!ci->attrs.count(id_PORT))
                log_error("pin buffer %s is not connected to a top-level port\n", ctx->nameOf(ci));
            std::string port = ci->attrs.at(id_PORT).as_string();
            if (!port2pin.count(port))
                log_error("no location assignment for port %s\n", port.c_str());
            // The pad side has no wire in the database; the bel is the pad.
            ci->disconnectPort(id_PAD);
            ci->ports.erase(id_PAD);
            std::string pin = port2pin.at(port);
            if (!pin2bel.count(pin))
                log_error("port %s: PIN_%s is not a user I/O pin\n", port.c_str(), pin.c_str());
            BelId bel = pin2bel.at(pin);
            if (!ctx->checkBelAvail(bel))
                log_error("port %s: PIN_%s is already used\n", port.c_str(), pin.c_str());
            ctx->bindBel(bel, ci, STRENGTH_LOCKED);
            log_info("    %s -> PIN_%s\n", port.c_str(), pin.c_str());
        }
    }

    // Constants have no driver in the fabric: LUT inputs are folded into
    // the mask, output pins get a constant output setting, register data
    // gets a LUT with a constant mask.
    void pack_constants()
    {
        std::vector<IdString> trim_cells, trim_nets;
        std::vector<std::pair<CellInfo *, bool>> const_ff;
        for (auto &net : ctx->nets) {
            NetInfo *ni = net.second.get();
            if (!ni->driver.cell || !ni->driver.cell->type.in(ctx->id("GND"), ctx->id("VCC")))
                continue;
            bool value = ni->driver.cell->type == ctx->id("VCC");
            std::vector<PortRef> users;
            for (auto &u : ni->users)
                users.push_back(u);
            for (auto &usr : users) {
                CellInfo *ci = usr.cell;
                if (ci->type == id_LCELL_COMB && (usr.port.in(id_DATAA, id_DATAB, id_DATAC, id_DATAD) || usr.port == id_CIN)) {
                    // CIN only acts as LUT input C (and the carry LUT's C);
                    // folding it makes both independent of the hardware cin.
                    int bit = usr.port == id_CIN ? 2
                                                 : (usr.port == id_DATAA ? 0 : usr.port == id_DATAB ? 1 : usr.port == id_DATAC ? 2 : 3);
                    uint64_t mask = ci->params.at(id_LUT_MASK).as_int64();
                    uint64_t out = 0;
                    for (int idx = 0; idx < 16; idx++) {
                        int src = value ? (idx | (1 << bit)) : (idx & ~(1 << bit));
                        out |= ((mask >> src) & 1) << idx;
                    }
                    ci->setParam(id_LUT_MASK, Property(out, 16));
                    ci->disconnectPort(usr.port);
                } else if (ci->type == id_CY2_OBUF && usr.port == id_DATAOUT) {
                    ci->setParam(id_OUT_CONST, Property(value ? 1 : 0, 1));
                    ci->disconnectPort(usr.port);
                } else if (ci->type == id_LCELL_FF && usr.port == id_DATAIN) {
                    ci->disconnectPort(usr.port);
                    const_ff.emplace_back(ci, value);
                } else {
                    log_error("constant %d on %s.%s is not supported\n", value, ctx->nameOf(ci), ctx->nameOf(usr.port));
                }
            }
            trim_cells.push_back(ni->driver.cell->name);
            trim_nets.push_back(ni->name);
        }
        for (IdString c : trim_cells)
            ctx->cells.erase(c);
        for (IdString n : trim_nets)
            ctx->nets.erase(n);
        for (auto &cf : const_ff) {
            CellInfo *lut = ctx->createCell(ctx->idf("%s$const", ctx->nameOf(cf.first)), id_LCELL_COMB);
            add_lut_ports(lut);
            lut->setParam(id_LUT_MASK, Property(cf.second ? 0xFFFF : 0x0000, 16));
            NetInfo *n = ctx->createNet(ctx->idf("%s$const_net", ctx->nameOf(cf.first)));
            lut->connectPort(id_COMBOUT, n);
            cf.first->connectPort(id_DATAIN, n);
        }
    }

    void add_lut_ports(CellInfo *lut)
    {
        for (auto p : lut_inputs)
            lut->addInput(p);
        lut->addInput(id_CIN);
        lut->addOutput(id_COMBOUT);
        lut->addOutput(id_COUT);
    }

    dict<IdString, CellInfo *> ff_of_lut;

    // Every register sits beside the LUT that drives it (the only register
    // data path modelled); registers without one get a pass-through LUT,
    // as Quartus' `~feeder` cells.
    void pack_lut_ff()
    {
        std::vector<CellInfo *> ffs;
        for (auto &cell : ctx->cells)
            if (cell.second->type == id_LCELL_FF)
                ffs.push_back(cell.second.get());
        int feeders = 0;
        for (CellInfo *ff : ffs) {
            NetInfo *d = ff->getPort(id_DATAIN);
            if (!d)
                log_error("register %s has no data input\n", ctx->nameOf(ff));
            CellInfo *drv = d->driver.cell;
            if (!(drv && drv->type == id_LCELL_COMB && d->driver.port == id_COMBOUT && !ff_of_lut.count(drv->name))) {
                CellInfo *lut = ctx->createCell(ctx->idf("%s$feeder", ctx->nameOf(ff)), id_LCELL_COMB);
                add_lut_ports(lut);
                lut->setParam(id_LUT_MASK, Property(0xAAAA, 16)); // COMBOUT = DATAA
                NetInfo *n = ctx->createNet(ctx->idf("%s$feeder_net", ctx->nameOf(ff)));
                ff->disconnectPort(id_DATAIN);
                lut->connectPort(id_DATAA, d);
                lut->connectPort(id_COMBOUT, n);
                ff->connectPort(id_DATAIN, n);
                drv = lut;
                feeders++;
            }
            ff_of_lut[drv->name] = ff;
            drv->cluster = drv->name;
            ff->cluster = drv->name;
            ff->constr_x = 0;
            ff->constr_y = 0;
            ff->constr_z = 1;
            ff->constr_abs_z = false;
            drv->constr_children.push_back(ff);
        }
        log_info("Packed %d registers with their LUTs (%d feeder LUTs).\n", int(ffs.size()), feeders);
    }

    // Carry chains: LE k -> k+1 in a LAB, LE 15 -> LE 0 of the LAB below.
    // A chain starts at LE 0.
    void pack_carry()
    {
        int chains = 0;
        for (auto &cell : ctx->cells) {
            CellInfo *ci = cell.second.get();
            if (ci->type != id_LCELL_COMB)
                continue;
            NetInfo *cin = ci->getPort(id_CIN);
            if (cin && cin->driver.cell)
                continue; // not a chain head
            NetInfo *cout = ci->getPort(id_COUT);
            if (!cout || cout->users.empty())
                continue;
            std::vector<CellInfo *> chain{ci};
            while (true) {
                NetInfo *co = chain.back()->getPort(id_COUT);
                if (!co || co->users.empty())
                    break;
                if (co->users.entries() != 1 || (*co->users.begin()).port != id_CIN)
                    log_error("carry out of %s must feed exactly one carry in\n", ctx->nameOf(chain.back()));
                chain.push_back((*co->users.begin()).cell);
            }
            for (CellInfo *c : chain)
                if (c->getPort(id_DATAC) && c->getPort(id_CIN))
                    log_error("%s uses both DATAC and CIN\n", ctx->nameOf(c));
            // Re-root: the head carries the whole chain and every paired
            // register.
            ci->cluster = ci->name;
            ci->constr_abs_z = true;
            ci->constr_z = 0;
            std::vector<CellInfo *> children;
            for (size_t i = 0; i < chain.size(); i++) {
                CellInfo *c = chain[i];
                int dy = -int(i / 16), z = 2 * int(i % 16);
                if (i > 0) {
                    c->cluster = ci->name;
                    c->constr_x = 0;
                    c->constr_y = dy;
                    c->constr_z = z;
                    c->constr_abs_z = true;
                    children.push_back(c);
                }
                for (CellInfo *ff : c->constr_children) {
                    ff->cluster = ci->name;
                    ff->constr_x = 0;
                    ff->constr_y = dy;
                    ff->constr_z = z + 1;
                    ff->constr_abs_z = true;
                    children.push_back(ff);
                }
                if (i > 0)
                    c->constr_children.clear();
            }
            ci->constr_children = children;
            normalise_carry_polarity(chain);
            chains++;
        }
        log_info("Packed %d carry chains.\n", chains);
    }

    // Quartus re-expresses every carry chain with the carry inverted after
    // each odd chain position (observed: counters from quartus_map, and our
    // own chains fed back to it as an atom netlist come out rewritten so),
    // and in the last LE copies the sum into the unused carry half. Both
    // forms compute the same thing; using Quartus' makes the bitstream
    // byte-comparable with what it assembles from the same placement.
    void normalise_carry_polarity(const std::vector<CellInfo *> &chain)
    {
        for (size_t i = 0; i < chain.size(); i++) {
            CellInfo *c = chain[i];
            uint64_t mask = c->params.at(id_LUT_MASK).as_int64();
            bool in_inv = i >= 2 && i % 2 == 0, out_inv = i % 2 == 1;
            uint64_t out = 0;
            for (int idx = 0; idx < 16; idx++) {
                uint64_t v = (mask >> (in_inv ? idx ^ 4 : idx)) & 1;
                if (idx < 8 && out_inv)
                    v ^= 1;
                out |= v << idx;
            }
            NetInfo *co = c->getPort(id_COUT);
            if (i + 1 == chain.size() && (!co || co->users.empty()))
                out = (out & 0xFF00) | (out >> 8);
            c->setParam(id_LUT_MASK, Property(out, 16));
        }
    }

    void pack_clocks()
    {
        // INCLK wire of every clock control -> its bel.
        dict<WireId, BelId> inclk;
        for (BelId bel : ctx->getBels())
            if (ctx->getBelType(bel) == id_CLKCTRL)
                inclk[ctx->getBelPinWire(bel, id_INCLK)] = bel;

        std::vector<NetInfo *> clocks;
        for (auto &net : ctx->nets) {
            NetInfo *ni = net.second.get();
            for (auto &usr : ni->users)
                if (usr.cell->type == id_LCELL_FF && usr.port == id_CLK) {
                    clocks.push_back(ni);
                    break;
                }
        }
        for (NetInfo *ni : clocks) {
            CellInfo *drv = ni->driver.cell;
            if (!drv || drv->type != id_CY2_IBUF)
                log_error("clock %s must come straight from a pin\n", ctx->nameOf(ni));
            // Nearest free clock control the pin reaches through the graph.
            WireId start = ctx->getBelPinWire(drv->bel, id_COMBOUT);
            std::queue<std::pair<WireId, int>> q;
            pool<WireId> seen{start};
            q.emplace(start, 0);
            BelId found;
            while (!q.empty() && found == BelId()) {
                auto [w, depth] = q.front();
                q.pop();
                if (inclk.count(w) && ctx->checkBelAvail(inclk.at(w))) {
                    found = inclk.at(w);
                    break;
                }
                if (depth > 8)
                    continue;
                for (PipId p : ctx->getPipsDownhill(w)) {
                    WireId d = ctx->getPipDstWire(p);
                    if (seen.insert(d).second)
                        q.emplace(d, depth + 1);
                }
            }
            if (found == BelId())
                log_error("no free global clock control reachable from %s\n", ctx->nameOf(drv));
            CellInfo *cc = ctx->createCell(ctx->idf("%s$clkctrl", ctx->nameOf(ni)), id_CLKCTRL);
            cc->addInput(id_INCLK);
            cc->addOutput(id_OUTCLK);
            NetInfo *g = ctx->createNet(ctx->idf("%s$global", ctx->nameOf(ni)));
            std::vector<PortRef> users;
            for (auto &u : ni->users)
                users.push_back(u);
            for (auto &usr : users)
                if (usr.cell->type == id_LCELL_FF && usr.port == id_CLK) {
                    usr.cell->disconnectPort(id_CLK);
                    usr.cell->connectPort(id_CLK, g);
                }
            cc->connectPort(id_INCLK, ni);
            cc->connectPort(id_OUTCLK, g);
            ctx->bindBel(found, cc, STRENGTH_LOCKED);
            log_info("    clock %s -> %s\n", ctx->nameOf(ni), ctx->nameOfBel(found));
        }
    }

    // --- placement validity ---

    bool isBelLocationValid(BelId bel, bool explain_invalid) const override
    {
        IdString t = ctx->getBelType(bel);
        if (!t.in(id_LCELL_COMB, id_LCELL_FF))
            return true;
        Loc l = ctx->getBelLocation(bel);
        // A register's data input is its LUT's output wire.
        int k = l.z / 2;
        const CellInfo *ff = ctx->getBoundBelCell(ctx->getBelByLocation(Loc(l.x, l.y, 2 * k + 1)));
        if (ff) {
            const CellInfo *lut = ctx->getBoundBelCell(ctx->getBelByLocation(Loc(l.x, l.y, 2 * k)));
            if (!lut || lut->getPort(id_COMBOUT) != ff->getPort(id_DATAIN))
                return false;
        }
        // Two LAB clocks.
        const NetInfo *clk[2] = {nullptr, nullptr};
        for (int i = 0; i < 16; i++) {
            const CellInfo *c = ctx->getBoundBelCell(ctx->getBelByLocation(Loc(l.x, l.y, 2 * i + 1)));
            if (!c)
                continue;
            const NetInfo *n = c->getPort(id_CLK);
            if (n == clk[0] || n == clk[1])
                continue;
            if (!clk[0])
                clk[0] = n;
            else if (!clk[1])
                clk[1] = n;
            else {
                if (explain_invalid)
                    log_nonfatal_error("LAB X%dY%d needs more than two clocks\n", l.x, l.y);
                return false;
            }
        }
        return true;
    }

    IdString getBelBucketForCellType(IdString cell_type) const override
    {
        if (cell_type.in(id_CY2_IBUF, id_CY2_OBUF))
            return id_IOE;
        return cell_type;
    }

    bool isValidBelForCellType(IdString cell_type, BelId bel) const override
    {
        IdString bel_type = ctx->getBelType(bel);
        if (bel_type == id_IOE)
            return cell_type.in(id_CY2_IBUF, id_CY2_OBUF);
        return bel_type == cell_type;
    }

  private:
    HimbaechelHelpers h;
};

struct Cyclone2Arch : HimbaechelArch
{
    Cyclone2Arch() : HimbaechelArch("cyclone2") {};
    bool match_device(const std::string &device) override { return device == "EP2C35F672C6"; }
    std::unique_ptr<HimbaechelAPI> create(const std::string &device) override
    {
        return std::make_unique<Cyclone2Impl>();
    }
} cyclone2Arch;

} // namespace

NEXTPNR_NAMESPACE_END
