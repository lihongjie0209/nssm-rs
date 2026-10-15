LanguageNames =
(
English=0x0409:MSG00409
French=0x40C:MSG0040C
Italian=0x410:MSG00410
)

MessageId = 1001
SymbolicName = NSSM_EVENT_DISPATCHER_FAILED
Severity = Error
Language = English
StartServiceCtrlDispatcher() failed:
%1
.
Language = French
Erreur en tentant de connecter la tâche principale du service au gestionnaire de services Windows.
StartServiceCtrlDispatcher() a échoué:
%1
.
Language = Italian
Chiamata a StartServiceCtrlDispatcher() fallita:
%1
.

MessageId = 1002
SymbolicName = NSSM_EVENT_OPENSCMANAGER_FAILED
Severity = Error
Language = English
Unable to connect to service manager!
Perhaps you need to be an administrator...
.
Language = French
Connexion impossible au gestionnaire de services!
Il vous manque peut-être des droits d'administrateur.
.
Language = Italian
Impossibile connettersi al Service Manager!
Probabilmente sono necessari permessi di amministratore...
.

MessageId = 1003
SymbolicName = NSSM_EVENT_OUT_OF_MEMORY
Severity = Error
Language = English
Out of memory for %1 in %2!
.
Language = French
Mémoire insuffisante pour %1 dans %2!
.
Language = Italian
Memoria insufficiente per %1 in %2!
.

MessageId = 1004
SymbolicName = NSSM_EVENT_GET_PARAMETERS_FAILED
Severity = Error
Language = English
Failed to get startup parameters for service %1.
.
Language = French
Paramètres de démarrage non trouvés pour le service %1.
.
Language = Italian
Impossibile ottenere i parametri di avvio per il servizio %1.
.

MessageId = 1005
SymbolicName = NSSM_EVENT_REGISTERSERVICECTRLHANDER_FAILED
Severity = Error
Language = English
RegisterServiceCtrlHandlerEx() failed:
%1
.
Language = French
Échec de l'enregistrement de la fonction de gestion des requêtes étendues de contrôle du service.
RegisterServiceCtrlHandlerEx() a échoué:
%1
.
Language = Italian
Chiamata a RegisterServiceCtrlHandlerEx() fallita:
%1
.

MessageId = 1006
SymbolicName = NSSM_EVENT_START_SERVICE_FAILED
Severity = Error
Language = English
Can't start %1 for service %2.
Error code: %3.
.
Language = French
Impossible de démarrer %1 pour le service %2.
Code erreur: %3.
.
Language = Italian
Impossibile avviare %1 per il servizio %2.
Codice errore: %3.
.

MessageId = 1007
SymbolicName = NSSM_EVENT_RESTART_SERVICE_FAILED
Severity = Warning
Language = English
Failed to restart %1 for service %2.
Sleeping...
.
Language = French
Impossible de redémarrer %1 pour le service %2.
Mise en sommeil...
.
Language = Italian
Impossibile riavviare %1 per il servizio %2.
In stato di attesa...
.

MessageId = 1008
SymbolicName = NSSM_EVENT_STARTED_SERVICE
Severity = Informational
Language = English
Started %1 %2 for service %3 in %4.
.
Language = French
Démarrage réussi de %1 %2 pour le service %3 depuis le répertoire %4.
.
Language = Italian
Avviati %1 %2 per il servizio %3 in %4.
.

MessageId = 1009
SymbolicName = NSSM_EVENT_REGISTERWAITFORSINGLEOBJECT_FAILED
Severity = Warning
Language = English
Service %1 may claim to be still running when %2 exits.
RegisterWaitForSingleObject() failed:
%3
.
Language = French
Le service %1 peut indiquer être toujours actif lorsque %2 se terminera.
RegisterWaitForSingleObject() a échoué:
%3
.
Language = Italian
Servizio %1 potrebbe indicare di essere ancora in esecuzione quando %2 termina.
Chiamata a RegisterWaitForSingleObject() fallita:
%3
.

MessageId = 1010
SymbolicName = NSSM_EVENT_CREATEPROCESS_FAILED
Severity = Error
Language = English
Failed to start service %1.  Program %2 couldn't be launched.
CreateProcess() failed:
%3
.
Language = French
Échec du démarrage du service %1. Le programme %2 n'a pas pu être lancé.
CreateProcess() a échoué:
%3
.
Language = Italian
Impossibile avviare il servizio %1.  Il programma %2 non può essere avviato.
Chiamata a CreateProcess() fallita:
%3
.

MessageId = 1011
SymbolicName = NSSM_EVENT_TERMINATEPROCESS
Severity = Informational
Language = English
Killing process %2 because service %1 is stopping.
.
Language = French
Arrêt forcé du processus %2 du fait de l'arrêt du service %1.
.
Language = Italian
Terminazione del processo %2 in quanto il servizio %1 sta terminando.
.

MessageId = 1012
SymbolicName = NSSM_EVENT_PROCESS_ALREADY_STOPPED
Severity = Informational
Language = English
Requested stop of service %1.  No action is required as program %2 is not running.
.
Language = French
Arrêt requis du service %1. Aucune action n'est requise car le programme %2 n'est pas en cours d'exécution.
.
Language = Italian
Richiesto l'arresto del servizio %1. Nessuna azione necessaria in quanto il programma %2 non è in esecuzione.
.

MessageId = 1013
SymbolicName = NSSM_EVENT_ENDED_SERVICE
Severity = Informational
Language = English
Program %1 for service %2 exited with return code %3.
.
Language = French
Le programme %1 pour le service %2 s'est arrêté avec code retour %3.
.
Language = Italian
Il programma %1 per il servizio %2 è terminato con codice errore %3.
.

MessageId = 1014
SymbolicName = NSSM_EVENT_EXIT_RESTART
Severity = Informational
Language = English
Service %1 action for exit code %2 is %3.
Attempting to restart %4.
.
Language = French
L'action prévue du service %1 pour le code retour %2 est: %3.
Tentative de redémarrage de %4.
.
Language = Italian
L'azione per il servizio %1, codice di uscita %2, è %3.
Tentativo di riavvio %4.
.

MessageId = 1015
SymbolicName = NSSM_EVENT_EXIT_IGNORE
Severity = Informational
Language = English
Service %1 action for exit code %2 is %3.
No action will be taken to restart %4.
.
Language = French
L'action prévue du service %1 pour le code retour %2 est: %3.
Aucune action ne sera entreprise pour redémarrer %4.
.
Language = Italian
L'azione per il servizio %1, codice di uscita %2, è %3.
Nessuna azione sarà intrapresa per riavviare %4.
.

MessageId = 1016
SymbolicName = NSSM_EVENT_EXIT_REALLY
Severity = Informational
Language = English
Service %1 action for exit code %2 is %3.
Exiting.
.
Language = French
L'action prévue du service %1 pour le code retour %2 est: %3.
Le programme ne sera pas redémarré.
.
Language = Italian
L'azione per il servizio %1, codice di uscita %2, è %3.
Avvio uscita.
.

MessageId = 1017
SymbolicName = NSSM_EVENT_OPENKEY_FAILED
Severity = Error
Language = English
Failed to open registry key HKLM\%1:
%2
.
Language = French
Échec de l'ouverture de la clé de registre HKLM\%1:
%2
.
Language = Italian
Impossibile aprire la chiave di registro HKLM\%1:
%2
.

MessageId = 1018
SymbolicName = NSSM_EVENT_QUERYVALUE_FAILED
Severity = Error
Language = English
Failed to read registry value %1:
%2
.
Language = French
Échec de l'ouverture de la valeur de registre %1:
%2
.
Language = Italian
Impossibile leggere la chiave di registro %1:
%2
.

MessageId = 1019
SymbolicName = NSSM_EVENT_SETVALUE_FAILED
Severity = Error
Language = English
Failed to write registry value %1:
%2
.
Language = French
Échec de l'écriture de la valeur de registre %1:
%2
.
Language = Italian
Impossibile scrivere la chiave di registro %1:
%2
.

MessageId = 1020
SymbolicName = NSSM_EVENT_EXIT_UNCLEAN
Severity = Informational
Language = English
Service %1 action for exit code %2 is %3.
Exiting.
.
Language = French
L'action prévue du service %1 pour le code retour %2 est: %3.
Le programme s'est terminé de manière impropre.
.
Language = Italian
L'azione per il servizio %1, codice di uscita %2, è %3.
Avvio terminazione.
.

MessageId = 1021
SymbolicName = NSSM_EVENT_GRACEFUL_SUICIDE
Severity = Informational
Language = English
Service %1 application %2 exited with exit code 0 but the default exit action is %3.
Honouring the %4 action would result in the service being flagged as failed and subject to recovery actions.
The service will instead be stopped gracefully.  To suppress this message, explicitly configure the exit action for exit code 0 to either %5 or %6.
.
Language = French
L'application %2 du service %1 s'est terminée sur un code retour 0.  Par défaut, lorsque l'application se termine, l'action suivante est configurée: %3.
Exécuter cette action %4 ferait que le service serait marqué en échec et sujet à des actions de récupération.
Donc, pour éviter cette situation, le service sera arrêté normalement.
Pour supprimer le présent message, configurez explicitement l'action de sortie pour le code retour 0 à %5 ou %6.
.
Language = Italian
Servizio %1 applicazione %2 è uscita con codice 0 ma l'azione di uscita di default è %3.
In base all'azione %4 il servizio andrebbe considerato fallito e soggetto ad azioni di ripristino.
Il servizio verrà invece terminato normalmente. Per eliminare questo messaggio, impostare l'azione di uscita per il codice di uscita 0 su %5 o %6.
.

MessageId = 1022
SymbolicName = NSSM_EVENT_EXPANDENVIRONMENTSTRINGS_FAILED
Severity = Error
Language = English
Failed to expand registry value %1:
%2
.
Language = French
Erreur lors de l'expansion des variables d'environnement dans la valeur de registre %1:
%2
.
Language = Italian
Impossibile espandere la chiave di registro %1:
%2
.

MessageId = 1023
SymbolicName = NSSM_EVENT_KILLING
Severity = Informational
Language = English
Killing process tree of process %2 for service %1 with exit code %3
.
Language = French
Interruption du processus %2 et de ses processus-fils pour le service %1. Code retour = %3
.
Language = Italian
Terminazione dell'albero di processo %2 per il servizio %1 con codice di uscita %3
.

MessageId = 1024
SymbolicName = NSSM_EVENT_CREATETOOLHELP32SNAPSHOT_PROCESS_FAILED
Severity = Error
Language = English
Failed to create snapshot of running processes when terminating service %1:
%2
.
Language = French
Impossible de créer un instantané des processus en cours d'exécution lors de l'arrêt du service %1:
%2
.
Language = Italian
Impossibile creare uno snapshot dei processi in esecuzione durante l'arresto del servizio %1!
%2
.

MessageId = 1025
SymbolicName = NSSM_EVENT_PROCESS_ENUMERATE_FAILED
Severity = Error
Language = English
Failed to enumerate running processes when terminating service %1:
%2
.
Language = French
Impossible d'énumérer les processus en cours d'exécution lors de l'arrêt du service %1:
%2
.
Language = Italian
Impossibile enumerare i processi in esecuzione durante la terminazione del servizio %1.
%2
.

MessageId = 1026
SymbolicName = NSSM_EVENT_OPENPROCESS_FAILED
Severity = Error
Language = English
Failed to open process handle for process with PID %1 when terminating service %2:
%3
.
Language = French
Échec à l'ouverture du handle de processus ayant le PID %1 lors de l'arrêt du service %2:
%3
.
Language = Italian
Impossibile aprire l'handle di proceso con PID %1 durante la terminazione del servizio %2.
%3
.

MessageId = 1027
SymbolicName = NSSM_EVENT_KILL_PROCESS_TREE
Severity = Informational
Language = English
Killing PID %1 in process tree of PID %2 because service %3 is stopping.
.
Language = French
Arrêt forcé du processus avec PID %1 (processus enfant du processus avec PID %2) résultant de l'arrêt du service %3.
.
Language = Italian
Terminazione del PID %1 nell'albero di processo con PID %2 in quanto il servizio %3 è in fase di terminazione.
.

MessageId = 1028
SymbolicName = NSSM_EVENT_TERMINATEPROCESS_FAILED
Severity = Error
Language = English
Failed to terminate process with PID %1 for service %2:
%3
.
Language = French
Impossible d'arrêter le processus avec PID %1 pour le service %2:
%3
.
Language = Italian
Impossibile terminare il processo con PID %1 per il servizio %2:
%3
.

MessageId = 1029
SymbolicName = NSSM_EVENT_NO_FLAGS
Severity = Warning
Language = English
Registry key %1 is unset for service %2.
No flags will be passed to %3 when it starts.
.
Language = French
La clé de registre %1 n'est pas définie pour le service %2.
Aucune option ne sera transmise à %3 lorsqu'il démarrera.
.
Language = Italian
La chiave di registro %1 non è impostata per il servizio %2.
Nessun argomento sarà passato a %3 in fase di avvio.
.

MessageId = 1030
SymbolicName = NSSM_EVENT_NO_DIR
Severity = Warning
Language = English
Registry key %1 is unset for service %2.
Assuming startup directory %3.
.
Language = French
La clé de registre %1 n'est pas définie pour le service %2.
Le répertoire de démarrage sera supposé être: %3.
.
Language = Italian
La chiave di registro %1 non è impostata per il servizio %2.
Cartella di avvio predefinita: %3.
.

MessageId = 1031
SymbolicName = NSSM_EVENT_NO_DIR_AND_NO_FALLBACK
Severity = Error
Language = English
Registry key %1 is unset for service %2.
Additionally, ExpandEnvironmentStrings("%%SYSTEMROOT%%") failed when trying to choose a fallback startup directory.
.
Language = French
La clé de registre %1 n'est pas définie pour le service %2.
De surcroît, l'expansion de la variable d'environnement "%%SYSTEMROOT%%" a échoué lors de la détermination d'un répertoire de démarrage de secours.
.
Language = Italian
La chiave di registro %1 non è impostata per il servizio %2.
Inoltre, la chiamata a ExpandEnvironmentStrings("%%SYSTEMROOT%%") è fallita in fase di scelta cartella alternativa.
.

MessageId = 1032
SymbolicName = NSSM_EVENT_CREATETOOLHELP32SNAPSHOT_THREAD_FAILED
Severity = Error
Language = English
Failed to create snapshot of running threads when terminating service %1:
%2
.
Language = French
Impossible de créer un instantané des threads en cours d'exécution lors de l'arrêt du service %1:
%2
.
Language = Italian
Impossibile creare uno snapshot dei thread attivi durante la fase di terminazione del servizio %1:
%2
.

MessageId = 1033
SymbolicName = NSSM_EVENT_THREAD_ENUMERATE_FAILED
Severity = Error
Language = English
Failed to enumerate running threads when terminating service %1:
%2
.
Language = French
Impossible d'énumérer les tâches (threads) en cours d'exécution lors de l'arrêt du service %1:
%2
.
Language = Italian
Impossibile enumerare i thread attivi durante la fase di terminazione del servizio %1:
%2
.

MessageId = 1034
SymbolicName = NSSM_EVENT_THROTTLED
Severity = Warning
Language = English
Service %1 ran for less than %2 milliseconds.
Restart will be delayed by %3 milliseconds.
.
Language = French
Le service %1 est resté actif durant moins de %2 millisecondes.
Son redémarrage sera retardé de %3 millisecondes.
.
Language = Italian
Il servizio %1 è rimasto in esecuzione per meno di %2 millisecondi.
Il riavvio verrà posticipato di %3 millisecondi.
.

MessageId = 1035
SymbolicName = NSSM_EVENT_RESET_THROTTLE
Severity = Informational
Language = English
Request to resume service %1.  Throttling of restart attempts will be reset.
.
Language = French
Demande de redémarrage du service %1.  La régulation des tentatives de redémarrage sera réinitialisée.
.
Language = Italian
Richiesta di riavvio per il servizio %1. Il meccanismo di regolazione della pausa di riavvio verrà resettato.
.

MessageId = 1036
SymbolicName = NSSM_EVENT_BOGUS_THROTTLE
Severity = Warning
Language = English
The registry value %2, used to specify the minimum number of milliseconds which must elapse before service %1 is considered to have started successfully, was not of type REG_DWORD.  The default time of %3 milliseconds will be used.
.
Language = French
La valeur de registre %2, indiquant le nombre minimal de millisecondes devant s'écouler avant que le service %1 soit considéré comme ayant démarré avec succès,
n'était pas du type REG_DWORD.  Une durée de %3 millisecondes sera utilisée par défaut.
.
Language = Italian
La chiave di registro %2, utilizzata per specificare il minimo numero di millisecondi che devono passare prima che il servizio %1 sia considerato avviato correttamente, non è di tipo REG_DWORD.
Verrà usato un tempo di default pari a %3 ms.
.

MessageId = 1037
SymbolicName = NSSM_EVENT_CREATEWAITABLETIMER_FAILED
Severity = Warning
Language = English
Failed to create waitable timer for service %1:
%2
Throttled restarts will not be interruptible.
.
Language = French
Impossible de créer un déclenchement temporisé ("waitable timer") pour le service %1:
%2
Les redémarrages régulés ne pourront pas être interrompus.
.
Language = Italian
Impossibile creare un timer per il servizio %1:
%2
Il meccanismo di regolazione della pausa di riavvio non sarà interrompibile.
.

MessageId = 1038
SymbolicName = NSSM_EVENT_CREATEPROCESS_FAILED_INVALID_ENVIRONMENT
Severity = Error
Language = English
Failed to start service %1.  Program %2 couldn't be launched.
CreateProcess() failed with ERROR_INVALID_PARAMETER and a process environment was set in the %3 registry value.  It is likely that the environment was incorrectly specified.  %3 should be a REG_MULTI_SZ value comprising strings of the form KEY=VALUE.
.
Language = French
Échec de démarrage du service %1.  Le programme %2 n'a pas pu être lancé.
La fonction CreateProcess() a échoué sur une erreur ERROR_INVALID_PARAMETER et un environnement de processus a été défini dans la valeur de base de registre %3.
Il est vraisemblable que l'environnement a été spécifié de manière incorrecte.
%3 devrait être définie comme valeur REG_MULTI_SZ comprenant des chaînes sous la forme CLEF=VALEUR.
.
Language = Italian
Impossibile avviare il servizio %1. Il programma %2 non può essere avviato.
Chiamata a CreateProcess() fallita con ERROR_INVALID_PARAMETER e ambiente di processo impostato nella chiave di registro %3. E' probabile che l'ambiente si stato specificato in modo errato.
%3 dovrebbe essere un valore REG_MULTI_SZ con stringhe nella forma CHIAVE=VALORE.
.

MessageId = 1039
SymbolicName = NSSM_EVENT_INVALID_ENVIRONMENT_STRING_TYPE
Severity = Warning
Language = English
Environment declaration %1 for service %2 is not of type REG_MULTI_SZ and will be ignored.
.
Language = French
La déclaration de l'environnement %1 pour le service %2 n'est pas du type REG_MULTI_SZ.  Cette déclaration sera ignorée.
.
Language = Italian
Dichiarazione di ambiente %1 per il servizio %2 non è di tipo REG_MULTI_SZ e verrà quindi ignorata.
.

MessageId = 1040
SymbolicName = NSSM_EVENT_SERVICE_CONTROL_HANDLED
Severity = Informational
Language = English
Service %1 received %2 control, which will be handled.
.
Language = French
Le service %1 a reçu le code de contrôle %2, qui sera pris en compte.
.
Language = Italian
Il servizio %1 ha ricevuto l'evento di controllo %2, che sarà gestito.
.

MessageId = 1041
SymbolicName = NSSM_EVENT_SERVICE_CONTROL_NOT_HANDLED
Severity = Informational
Language = English
Service %1 received unsupported %2 control, which will not be handled.
.
Language = French
Le service %1 a reçu le code de contrôle %2, qui n'est pas géré.  Aucune action ne sera entreprise en réponse à cette demande.
.
Language = Italian
Il servizio %1 ha ricevuto l'evento di controllo non supportato %2, che non sarà gestito.
.

MessageId = 1042
SymbolicName = NSSM_EVENT_SERVICE_CONTROL_UNKNOWN
Severity = Informational
Language = English
Service %1 received unknown service control message %2, which will be ignored.
.
Language = French
Le service %1 a reçu le code de contrôle inconnu %2, qui sera donc ignoré.
.
Language = Italian
Il servizio %1 ha ricevuto un messaggio di controllo sconosciuto %2, che sarà ignorato.
.

MessageId = 1043
SymbolicName = NSSM_EVENT_SERVICE_CONFIG_FAILURE_ACTIONS_FAILED
Severity = Informational
Language = English
Error configuring service failure actions for service %1.  The service will not be subject to recovery actions if it exits gracefully with a non-zero exit code.
ChangeServiceConfig2() failed:
%2
.
Language = French
Erreur lors de la configuration des actions en cas d'échec du service %1.  Le service ne déclenchera aucune action de récupération s'il se termine normalement avec un code retour non nul.
ChangeServiceConfig2() a échoué:
.
Language = Italian
Errore di configurazione delle azioni di fallimento per il servizio %1. Il servizio non sarà soggetto ad azioni di ripristino nel caso termini in modo normale con un codice di uscita non nullo.
Chiamata a ChangeServiceConfig2() fallita:
%2
.

MessageId = 1044
SymbolicName = NSSM_EVENT_GETPROCESSTIMES_FAILED
Severity = Error
Language = English
GetProcessTimes() failed:
%1
.
Language = French
Échec de GetProcessTimes():
%1
.
Language = Italian
Chiamata a GetProcessTimes() fallita:
%1
.

MessageId = 1045
SymbolicName = NSSM_EVENT_ATTACHCONSOLE_FAILED
Severity = Error
Language = English
Error attaching to console for service %1.
AttachConsole() failed:
%2
.
Language = French
Error attaching to console for service %1.
AttachConsole() a échoué:
%2
.
Language = Italian
Errore di collegamento alla console del servizio %1.
Chiamata a AttachConsole() fallita:
%2
.

MessageId = 1046
SymbolicName = NSSM_EVENT_SETCONSOLECTRLHANDLER_FAILED
Severity = Error
Language = English
Error setting null handler for Control-C events sent to service %1.
SetConsoleCtrlHandler() failed:
%2
.
Language = French
Error setting null handler for Control-C events sent to service %1.
SetConsoleCtrlHandler() a échoué:
%2
.
Language = Italian
Errore nella configurazione del gestore eventi "Control-C" inviati al servizio %1.
Chiamata a SetConsoleCtrlHandler() fallita:
%2
.

MessageId = 1047
SymbolicName = NSSM_EVENT_GENERATECONSOLECTRLEVENT_FAILED
Severity = Error
Language = English
Error generating Control-C event for service %1.
GenerateConsoleCtrlEvent() failed:
%2
.
Language = French
Error generating Control-C event for service %1.
GenerateConsoleCtrlEvent() a échoué:
%2
.
Language = Italian
Errore nella generazione dell'evento "Control-C" da inviare al servizio %1.
Chiamata a GenerateConsoleCtrlEvent() fallita:
%2
.

MessageId = 1048
SymbolicName = NSSM_EVENT_FREECONSOLE_FAILED
Severity = Warning
Language = English
Error detaching from console for service %1.
FreeConsole() failed:
%2
.
Language = French
Erreur lors de la déconnexion de la console pour le service %1.
FreeConsole() a échoué:
%2
.
Language = Italian
Errore durante il rilascio della console per il servizio %1.
Chiamata a FreeConsole() fallita:
%2
.

MessageId = 1049
SymbolicName = NSSM_EVENT_CREATEFILE_FAILED
Severity = Error
Language = English
CreateFile() failed to open %1:
%2
.
Language = French
CreateFile() a échoué %1:
%2
.
Language = Italian
Chiamata a CreateFile() per aprire %1 fallita:
%2
.

MessageId = 1050
SymbolicName = NSSM_EVENT_DUPLICATEHANDLE_FAILED
Severity = Error
Language = English
Error duplicating the filehandle previously opened for %1 as %2.
DuplicateHandle() failed:
%3
.
Language = French
DuplicateHandle() a échoué (%1 -> %2):
%3
.
Language = Italian
Chiamata a DuplicateHandle() - (%1 -> %2) fallita:
%3
.

MessageId = 1051
SymbolicName = NSSM_EVENT_GET_OUTPUT_HANDLES_FAILED
Severity = Error
Language = English
Error setting up one or more I/O filehandles.  Service %1 will not be started.
.
Language = French
Erreur à la configuration d'un ou plusieurs handles d'E/S. Le service %1 ne sera pas démarré.
.
Language = Italian
Errore nella configurazione di uno o più I/O filehandles. Il servizio %1 non sarà avviato.
.

MessageId = 1052
SymbolicName = NSSM_EVENT_BOGUS_STOP_METHOD_SKIP
Severity = Warning
Language = English
The registry value %2, used to specify the method(s) by which %3 will skip when attempting to stop service %1, was not of type REG_DWORD.  All available methods will be used.
.
Language = French
La valeur de registre %2, utilisée pour spécifier les méthodes non utilisées par %3 lors de tentative d'arrêt du service %1 n'est pas du type REG_DWORD. Aucune méthode ne sera utilisée.
.
Language = Italian
La chiave di registro %2, usata per specificare i metodi da non usare per %3 nel tentativo di fermare il servizio %1, non è di tipo REG_DWORD. Tutti i metodi disponibili saranno usati.
.

MessageId = 1053
SymbolicName = NSSM_EVENT_PROCESS_STILL_ACTIVE
Severity = Warning
Language = English
The service %1 is stopping but PID %2 is still running.
Usually %3 will call TerminateProcess() as a last resort to ensure that the process is stopped but the registry value %4 has been set and not all process termination methods have been attempted.
It will no longer be possible to attempt to control the application and the service will report a stopped status.
.
Language = French
Le service %1 est en cours d'arrêt mais le processus de PID %2 est toujours en cours d'exécution.
Normalement %3 effectuera un appel à TerminateProcess() en dernier recours afin de s'assurer que le processus est arrété, mais la valeur de registre %4 a été configurée et toutes les méthodes d'arrêt n'ont pas été tentées.
Il ne sera plus possible de tenter de contrôler cette application, et le service sera indiqué comme arretté.
.
Language = Italian
Il servizio %1 è in fase di arresto ma il PID %2 è ancora attivo.
Normalmente %3 chiama TerminateProcess() come ultima possibilità per assicurare che il processo sia fermato ma la chiave di registro %4 è configurata e non tutti i metodi di terminazione sono stati tentati.
Non sarà più possibile gestire l'applicazione e il servizio sarà riportato come Arrestato.
.

MessageId = 1054
SymbolicName = NSSM_EVENT_LOADLIBRARY_FAILED
Severity = Warning
Language = English
Error loading the %1 DLL!
LoadLibrary() failed:
%2
.
Language = French
Erreur à l'ouverture de la DLL %1!
LoadLibrary() a échoué:
%2
.
Language = Italian
Errore apertura DLL %1!
Chiamata a LoadLibrary() fallita:
%2
.

MessageId = 1055
SymbolicName = NSSM_EVENT_GETPROCADDRESS_FAILED
Severity = Warning
Language = English
GetProcAddress(%1) failed:
%2
.
Language = French
GetProcAddress(%1) a échoué:
%2
.
Language = Italian
Chiamata a GetProcAddress(%1) fallita:
%2
.

MessageId = 1056
SymbolicName = NSSM_EVENT_BOGUS_KILL_CONSOLE_GRACE_PERIOD
Severity = Warning
Language = English
The registry value %2, used to specify the maximum number of milliseconds to wait for service %1 to stop after sending a Control-C event, was not of type REG_DWORD.  The default time of %3 milliseconds will be used.
.
Language = French
La valeur de registre %2, servant à spécifier en millisecondes le délai d'attente d'arrêt du service %1 lorsqu'un évênement de type Contrôle-C est reçu, n'est pas du type REG_DWORD. La valeur par défaut de %3 millisecondes sera utilisée.
.
Language = Italian
La chiave di registro %2, usata per specificare quanto millisecondi attendere l'arresto del servizio %1 dopo l'invio di un evento "Control-C" non è di tipo REG_DWORD. Sarà usato un default di %3 millisecondi.
.

MessageId = 1057
SymbolicName = NSSM_EVENT_BOGUS_KILL_WINDOW_GRACE_PERIOD
Severity = Warning
Language = English
The registry value %2, used to specify the maximum number of milliseconds to wait for service %1 to stop after posting a WM_CLOSE message to windows managed by the application, was not of type REG_DWORD.  The default time of %3 milliseconds will be used.
.
Language = French
La valeur de registre %2, servant à spécifier en millisecondes le délai d'attente d'arrêt du service %1 lorsqu'un message WM_CLOSE est envoyé à une fenêtre gérée par l'application, n'est pas du type REG_DWORD. La valeur par défaut de %3 millisecondes sera utilisée.
.
Language = Italian
La chiave di registro %2, usata per specificare quanti millisecondi attendere l'arresto del servizio %1 dopo l'invio dei messaggi "WM_CLOSE" alle windows dell'applicazione non è di tipo REG_DWORD. Sarà usato un default di %3 millisecondi.
.

MessageId = 1058
SymbolicName = NSSM_EVENT_BOGUS_KILL_THREADS_GRACE_PERIOD
Severity = Warning
Language = English
The registry value %2, used to specify the maximum number of milliseconds to wait for service %1 to stop after posting a WM_QUIT message to the message queues of threads managed by the application, was not of type REG_DWORD.  The default time of %3 milliseconds will be used.
.
Language = French
La valeur de registre %2, servant à spécifier en millisecondes le délai d'attente d'arrêt du service %1 lorsqu'un message WM_QUIT est envoyé à une fenêtre gérée par l'application, n'est pas du type REG_DWORD. La valeur par défaut de %3 millisecondes sera utilisée.
.
Language = Italian
La chiave di registro %2, usata per specificare quanti millisecondi attendere l'arresto del servizio %1 dopo l'invio del messaggio "WM_QUIT" ai threads dell'applicazione non è di tipo REG_DWORD. Sarà usato un default di %3 millisecondi.
.

MessageId = 1059
SymbolicName = NSSM_EVENT_AWAITING_SHUTDOWN
Severity = Informational
Language = English
%1 has waited %3 of %5 milliseconds for the %2 service to exit.
Next update in %4 milliseconds.
.
Language = French
%1 a attendu %3 millisecondes sur %5 pour l'arrêt du service %2.
Prochaine mise à jour dans %4 millisecondes.
.
Language = Italian
%1 ha atteso %3 dei %5 millisecondi per l'arresto del servizio %2.
Prossimo aggiornamento in %4 millisecondi.
.

MessageId = 1060
SymbolicName = NSSM_EVENT_CREATETHREAD_FAILED
Severity = Error
Language = English
CreateThread() failed:
%1
.
Language = French
CreateThread() a échoué:
%1
.
Language = Italian
Chiamata a CreateThread() fallita:
%1
.

MessageId = 1061
SymbolicName = NSSM_EVENT_STARTUP_DELAY_TOO_LONG
Severity = Informational
Language = English
The minimum number of milliseconds which must pass before service %1 is considered to have been started successfully is set to %2.  Access to the Windows service control manager is blocked until the service updates its status, therefore %3 will wait a maximum of %4 milliseconds before reporting the service's state as running.  Service restart throttling will be enforced if the service runs for less than the full %2 milliseconds.
.
Language = French
Le temps minimum en millisecondes avant que le service %1 soit considéré comme démarré avec succès a été configuré à %2.  L'accès à la fenêtre de gestion du service est bloqué jusqu'à ce que le service mette à jour son statut, aussi %3 attendra au maximum %4 millisecondes avant d'indiquer que le service est démarré. Le service de redémarrage automatique de service sera activé si le service tourne pendant moins de %2 millisecondes.
.
Language = Italian
Il minimo numero di millisecondi da attendere perché %1 sia considerato avviato con successo è configurato a %2. L'accesso al gestore dei controlli dei servizi di Windows è bloccato finchè il servizio non aggiorna il suo stato, quindi %3 attenderà un massimo di %4 millisecondi prima di riportare lo stato del servizio come avviato. La funzione di riavvio ritardato sarà attivata se l'applicazione esce prima di %2 millisecondi.
.

MessageId = 1062
SymbolicName = NSSM_EVENT_SETENVIRONMENTVARIABLE_FAILED
Severity = Warning
Language = English
SetEnvironmentVariable(%1=%2) failed:
%3
.
Language = French
SetEnvironmentVariable(%1=%2) a échoué:
%3
.
Language = Italian
Chiamata a SetEnvironmentVariable(%1=%2) fallita:
.

MessageId = 1063
SymbolicName = NSSM_EVENT_ROTATE_FILE_FAILED
Severity = Error
Language = English
Failed to rotate output file %2 for service %1.
%3 failed for file %4:
%5
.
Language = French
Impossible d'effectuer la rotation de fichier de sortie %2 pour le service %1.
%3 échoué pour le fichier %4:
%5
.
Language = Italian
Impossibile ruotare l'output file %2 per il servizio %1.
%3 è fallita per il file %4:
%5
.

MessageId = 1064
SymbolicName = NSSM_EVENT_SERVICE_CONFIG_DESCRIPTION_FAILED
Severity = Informational
Language = English
Error setting description for service %1.
ChangeServiceConfig2() failed:
%2
.
Language = French
Erreur lors de la sauvegarde de la description du service %1.
ChangeServiceConfig2() a échoué:
%2
.
Language = Italian
Errore durante la configurazione della descrizione del servizio %1.
Chiamata a ChangeServiceConfig2() fallita:
%2
.

MessageId = 1065
SymbolicName = NSSM_EVENT_SERVICE_CONFIG_DELAYED_AUTO_START_INFO_FAILED
Severity = Informational
Language = English
Error configuring delayed startup for service %1.  The service will start automatically.
ChangeServiceConfig2() failed:
%2
.
Language = French
Erreur lors de la sauvegarde du démarrage retardé pour le service %1. Le service démarrera automatiquement.
ChangeServiceConfig2() failed:
%2
.
Language = Italian
Errore durante la configurazione dell'avvio ritardato del servizio %1. Il servizio si avvierà automaticamente.
Chiamata a ChangeServiceConfig2() fallita:
%2
.

MessageId = 1066
SymbolicName = NSSM_EVENT_BOGUS_PRIORITY
Severity = Informational
Language = English
The registry value %2, used to specify the priority class for service %1, was not valid.
%2 should be of type REG_DWORD and correspond to a valid argument to the
SetPriorityClass() function.
Service %1 will be started with normal priority.
.
Language = French
La valeur de registre %2, utilisée pour spécifier la classe de priorité du service %1 est incorrecte.
%2 devrait être de type REG_DWORD et correspondre à une valeur de paramètre correcte pour la fonction
SetPriorityClass().
Le service %1 sera démarré avec la priorité normale.
.
Language = Italian
La chiave di registro %2, usata per specificare la classe di priorità per il servizio %1, non è valida.
%2 dovrebbe essere di tipo REG_DWORD e corrispondere ad un valido argomento per la funzione
SetPriorityClass().
Il servizio %1 sarà avviato con priorità "Normale".
.

MessageId = 1067
SymbolicName = NSSM_EVENT_BOGUS_AFFINITY_MASK
Severity = Warning
Language = English
Requested affinity range %2 is invalid.
Service %1 will be allowed to run on any CPU.
.
Language = French
La plage d'affinité requise %2 est incorrecte.
Le service %1 sera autorisé à tourner sur n'importe quelle UC.
.
Language = Italian
La affinità richiesta %2 non è valida.
Il servizio %1 potrà usare tutte le CPU.
.

MessageId = 1068
SymbolicName = NSSM_EVENT_EFFECTIVE_AFFINITY_MASK
Severity = Warning
Language = English
Requested processor affinity range %2 is not appropriate.
The maximal affinity range is %3 on this system.
Service %1 will run with an affinity range of %4.
.
Language = French
La plage d'affinité requise %2 est incorrecte.
La plage de valeur maximale d'affinité sur ce système est %3.
Le service %1 tournera avec une plage d'affinité de %4.
.
Language = Italian
Il range di affinità richiesto "%2" non è appropriato.
Il massimo range di affintà su questo sistema è %3.
Il servizio %1 sarà avviato con un range di affinità di %4.
.

MessageId = 1069
SymbolicName = NSSM_EVENT_GETPROCESSAFFINITYMASK_FAILED
Severity = Warning
Language = English
Failed to determine an appropriate affinity mask for service %1.
GetProcessAffinityMask(): %2
.
Language = French
Impossible de déterminer un masque d'affinité approprié pour le service %1.
GetProcessAffinityMask(): %2
.
Language = Italian
Impossibile determinare una maschera di affinità appropriata per il servizio %1.
GetProcessAffinityMask(): %2
.

MessageId = 1070
SymbolicName = NSSM_EVENT_SETPROCESSAFFINITYMASK_FAILED
Severity = Error
Language = English
Failed to set requested affinity mask for service %1.
SetProcessAffinityMask(): %2
.
Language = French
Impossible de configurer le service %1 avec la valeur requise pour le masque d'affinité.
SetProcessAffinityMask(): %2
.
Language = Italian
Impossibile configurare la maschera di affinità richiesta per il servizio %1.
SetProcessAffinityMask(): %2
.

MessageId = 1071
SymbolicName = NSSM_EVENT_BOGUS_RESTART_DELAY
Severity = Warning
Language = English
The registry value %2, used to specify the number of milliseconds by which restarts of service %1 will be delayed, was not of type REG_DWORD.
No mandatory delay will be employed.
.
Language = French
La valeur de registre %2, servant à spécifier en millisecondes le délai avant redémarrage du service %1 n'est pas du type REG_DWORD.
Aucune valeur de délai ne sera configurée.
.
Language = Italian
La chiave di registro %2, usata per specificare il minimo posticipo in millisecondi da applicare al riavvio del servizio %1, non è di tipo REG_DWORD.
Nessun posticipo minimo sarà considerato.
.

MessageId = 1072
SymbolicName = NSSM_EVENT_RESTART_DELAY
Severity = Informational
Language = English
Restart of service %1 will be delayed by %2 milliseconds.
.
Language = French
Le redémarrage du service %1 sera retardé de %2 millisecondes.
.
Language = Italian
Il riavvio del servizio %1 verrà posticipato di %2 millisecondi.
.

MessageId = 1073
SymbolicName = NSSM_EVENT_CREATEPIPE_FAILED
Severity = Error
Language = English
Failed to set up a pipe to read output from service %1.
Rotation of log file %2 will not be possible while the service is running.
CreatePipe(): %3
.
Language = French
Impossible de configurer le tube (pipe) pour la lecture de la sortie du service %1.
La rotation de fichier de log %2 ne sera pas possible pendant que le service tourne.
CreatePipe(): %3
.
Language = Italian
Impossibile configurare una pipe per ottenere l'output dal servizio %1.
La rotazione del file di log %2 mentre il servizio è in esecuzione non sarà possibile.
CreatePipe(): %3
.

MessageId = 1074
SymbolicName = NSSM_EVENT_READFILE_FAILED
Severity = Error
Language = English
Failed to read output for service %1.
If the error persists, no more data will be written to %2.
ReadFile(): %3
.
Language = French
Impossible de lire la sortie du service %1.
Si l'erreur persiste, aucune donnée supplémentaire ne pourra être écrite vers %2.
ReadFile(): %3
.
Language = Italian
Impossibile leggere l'output del servizio %1,
Se l'errore persiste, nessun dato di log sarà scritto in %2
ReadFile(): %3
.

MessageId = 1075
SymbolicName = NSSM_EVENT_WRITEFILE_FAILED
Severity = Error
Language = English
Failed to write output for service %1 to file %2.
If the error persists, some data may be lost.
WriteFile(): %3
.
Language = French
Impossible d'écrire la sortie du service %1 dans le fichier %2.
Si l'erreur persiste, des données pourraient être perdues.
WriteFile(): %3
.
Language = Italian
Impossibile scrivere l'output del servizio %1 nel file %2.
Se l'errore persiste, alcuni dati di log potrebbero andare persi.
WriteFile(): %3
.

MessageId = 1076
SymbolicName = NSSM_EVENT_SOMEBODY_SET_UP_US_THE_BOM
Severity = Warning
Language = English
Output from service %1 was detected as being in UTF-16 format but an attempt to write an appropriate byte order marker failed.
It is likely that subsequent attempts to write data to %2 will fail.  If they succeed, the file may not be recognised as being
in UTF-16 format by applications which attempt to read it.
WriteFile(): %3
.
Language = French
La sortie du service %1 a été détectée comme étant au format UTF-16, mais un essai d'écriture avec le marqueur d'indicateur d'ordre des octets (BOM) a échoué.
Il est probable que les tentatives ultérieures d'écriture des données vers %2 échouent. Si elles réussissent, le fichier peut néanmoins ne pas être identifié
comme étant au format UTF-16 par les applications tentant de le lire.
WriteFile(): %3
.
Language = Italian
L'output dal servizio %1 è di tipo UTF-16 ma il tentativo di memorizzare l'appropriato marcatore di byte-order è fallito.
E' probabile che i successivi tentativi di scrittura in %2 falliranno ma se avessero successo il file potrebbe non essere riconosciuto
come di tipo UTF-16 dalle applicazioni che tenteranno di leggerlo.
WriteFile(): %3
.

MessageId = 1077
SymbolicName = NSSM_EVENT_ROTATED
Severity = Informational
Language = English
Rotated output file %2 for service %1 to %3.
.
Language = French
Rotation du fichier de sortie %2 pour le service %1 vers %3.
.
Language = Italian
Rotazione del file di output %2 in %3 per il servizio %1.
.

MessageId = 1078
SymbolicName = NSSM_EVENT_AWAITING_SINGLE_HANDLE
Severity = Informational
Language = English
%1 has waited %3 of %5 milliseconds for the %2 handle.
Next update in %4 milliseconds.
.
Language = French
%1 has waited %3 of %5 milliseconds for the %2 handle.
Next update in %4 milliseconds.
.
Language = Italian
%1 has waited %3 of %5 milliseconds for the %2 handle.
Next update in %4 milliseconds.
.

MessageId = 1079
SymbolicName = NSSM_EVENT_PRESTART_HOOK_ABORT
Severity = Informational
Language = English
The %1/%2 hook for service %3 returned exit code %4.
Service startup will be aborted.
.
Language = French
The %1/%2 hook for service %3 returned exit code %4.
Service startup will be aborted.
.
Language = Italian
The %1/%2 hook for service %3 returned exit code %4.
Service startup will be aborted.
.

MessageId = 1080
SymbolicName = NSSM_EVENT_HOOK_CREATEPROCESS_FAILED
Severity = Informational
Language = English
Failed to run %1/%2 hook for service %3.  Program %4 couldn't be launched.
CreateProcess() failed:
%5
.
Language = French
Failed to run %1/%2 hook for service %3.  Program %4 couldn't be launched.
CreateProcess() failed:
%5
.
Language = Italian
Failed to run %1/%2 hook for service %3.  Program %4 couldn't be launched.
CreateProcess() failed:
%5
.

MessageId = 1081
SymbolicName = NSSM_EVENT_GET_HOOK_FAILED
Severity = Informational
Language = English
Failed to find a command for the %1/%2 hook for service %3 in the registry.
.
Language = French
Failed to find a command for the %1/%2 hook for service %3 in the registry.
.
Language = Italian
Failed to find a command for the %1/%2 hook for service %3 in the registry.
.
