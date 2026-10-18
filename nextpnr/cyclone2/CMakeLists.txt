# Out-of-tree uarch: the chip database is built outside CMake (`just
# chipdb`: gen_chipdb.py on the repo's db/EP2C35F672C6/chipdb.txt.xz, then
# bbasm), so nothing is built here but the code. nextpnr loads it with
# --chipdb.
set(SOURCES
    constids.inc
    gfxids.inc
    cyclone2.cc
)

add_nextpnr_himbaechel_microarchitecture(${uarch}
    CORE_SOURCES ${SOURCES}
)
