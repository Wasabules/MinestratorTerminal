; Hooks NSIS du service d'arrière-plan (docs/SERVICE.md).
;
; L'application s'installe TOUJOURS pour l'utilisateur courant (`installMode: "currentUser"`) :
; ni l'installation, ni les mises à jour, ni la désinstallation de l'app ne demandent de droits
; administrateur.
;
; Le « mode administrateur », c'est le service de supervision en arrière-plan (composant
; machine). L'installeur propose de l'activer et note le choix dans HKCU ; l'activation elle-même
; (élévation UAC, copie protégée du binaire sous Program Files, transmission de la clé API) a lieu
; dans l'application à la première connexion — le service a besoin de la clé API. Voir
; `minestrator_core::service::take_pending_setup` (même clé de registre).
;
; Désinstallation : si le service est installé, sa suppression demande une élévation (UAC).
; Mises à jour (updater, /UPDATE) : rien ici — la copie protégée du service se met à jour depuis
; l'application (« Mettre à jour le service »).
;
; Variables du modèle Tauri utilisées : $UpdateMode, $PassiveMode (installeur) et
; $DeleteAppDataCheckboxState (case « supprimer les données » du désinstalleur).

!define MST_DAEMON "minestrator-daemon.exe"
!define MST_SERVICE "MinestratorDaemon"
!define MST_PENDING_KEY "Software\MinestratorTerminal"
!define MST_PENDING_VALUE "ServiceSetupRequested"

; $0 ← 0 si le service est installé (code de sortie de `sc query`), sinon non nul.
!macro MST_QUERY_SERVICE
  nsExec::ExecToStack 'sc.exe query ${MST_SERVICE}'
  Pop $0 ; code de sortie
  Pop $1 ; sortie texte (ignorée)
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Push $0
  Push $1
  Push $2
  ; Mise à jour, mode passif ou silencieux : aucune question.
  ${If} $UpdateMode <> 1
  ${AndIf} $PassiveMode <> 1
  ${AndIfNot} ${Silent}
    !insertmacro MST_QUERY_SERVICE
    ${If} $0 <> 0
      ${If} $LANGUAGE = 1036 ; français
        StrCpy $2 "Activer la supervision en arrière-plan ?$\r$\n$\r$\nUn service système surveille vos serveurs (crash, seuils, expiration) même quand l'application est fermée ou la session déconnectée.$\r$\n$\r$\nIl demande les droits administrateur : la demande apparaîtra à votre première connexion dans l'application. L'application elle-même reste installée pour votre seul compte, sans droits particuliers.$\r$\n$\r$\nVous pourrez aussi l'activer plus tard dans Réglages → Service d'arrière-plan."
      ${Else}
        StrCpy $2 "Enable background supervision?$\r$\n$\r$\nA system service monitors your servers (crashes, thresholds, expiry) even when the app is closed or you are signed out.$\r$\n$\r$\nIt requires administrator rights: you will be asked at your first sign-in in the app. The app itself stays installed for your account only, without special rights.$\r$\n$\r$\nYou can also enable it later in Settings → Background service."
      ${EndIf}
      ${If} ${Cmd} `MessageBox MB_YESNO|MB_ICONQUESTION "$2" /SD IDNO IDYES`
        WriteRegDWORD HKCU "${MST_PENDING_KEY}" "${MST_PENDING_VALUE}" 1
      ${Else}
        DeleteRegValue HKCU "${MST_PENDING_KEY}" "${MST_PENDING_VALUE}"
      ${EndIf}
    ${EndIf}
  ${EndIf}
  Pop $2
  Pop $1
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Push $0
  Push $1
  ; Mise à jour : rien (le service n'exécute pas les fichiers de l'application).
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "${MST_PENDING_KEY}" "${MST_PENDING_VALUE}"
    DeleteRegKey /ifempty HKCU "${MST_PENDING_KEY}"
    !insertmacro MST_QUERY_SERVICE
    ${If} $0 = 0
    ${AndIf} ${FileExists} "$INSTDIR\${MST_DAEMON}"
      ${If} ${Silent}
        DetailPrint "Service d'arrière-plan conservé (désinstallation silencieuse : aucune élévation)."
      ${Else}
        DetailPrint "Suppression du service d'arrière-plan (droits administrateur)…"
        ClearErrors
        ${If} $DeleteAppDataCheckboxState = 1
          ; Service, clé machine, historique et journaux.
          ExecShellWait "runas" "$INSTDIR\${MST_DAEMON}" "uninstall --purge" SW_HIDE
        ${Else}
          ; Service et clé machine ; historique conservé.
          ExecShellWait "runas" "$INSTDIR\${MST_DAEMON}" "uninstall" SW_HIDE
        ${EndIf}
        ${If} ${Errors}
          DetailPrint "Élévation refusée : le service reste installé. Pour le retirer, en administrateur :"
          DetailPrint '"$PROGRAMFILES64\MinestratorTerminal\Service\${MST_DAEMON}" uninstall'
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${EndIf}
  Pop $1
  Pop $0
!macroend
