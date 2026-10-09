; Execute the real x86 helper through NSIS's Unicode stack ABI.
Unicode true
RequestExecutionLevel user
SilentInstall silent
SetCompressor zlib
!include LogicLib.nsh
!addplugindir "${HELPERDIR}"
OutFile "${TESTDIR}\helper-smoke.exe"
Section
  minidaw_nsis_utils::SemverCompare "1.2.3" "1.2.3"
  Pop $0
  ${If} $0 != 0
    SetErrorLevel 1
    Quit
  ${EndIf}
  minidaw_nsis_utils::SemverCompare "1.2.4" "1.2.3"
  Pop $0
  ${If} $0 != 1
    SetErrorLevel 2
    Quit
  ${EndIf}
  minidaw_nsis_utils::SemverCompare "1.2.3" "1.2.4"
  Pop $0
  ${If} $0 != -1
    SetErrorLevel 3
    Quit
  ${EndIf}
  minidaw_nsis_utils::StrReplace "MiniDAW {version} / {version}" "{version}" "0.1.0"
  Pop $0
  ${If} $0 != "MiniDAW 0.1.0 / 0.1.0"
    SetErrorLevel 4
    Quit
  ${EndIf}
  SetErrorLevel 0
SectionEnd
