# Out-of-tree uarch: the chip database is generated from the user's own
# Quartus device files at use time (`just chipdb`), so nothing is built here
# but the code. nextpnr loads it with --chipdb.
set(SOURCES
    constids.inc
    gfxids.inc
    cyclone2.cc
)

add_nextpnr_himbaechel_microarchitecture(${uarch}
    CORE_SOURCES ${SOURCES}
)
