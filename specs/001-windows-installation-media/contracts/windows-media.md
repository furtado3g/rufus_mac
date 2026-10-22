# Contract — CLI, GUI e preparação Windows

Contrato proposto; comandos novos só estarão disponíveis após implementação.

## CLI

Preservar `list`, `analyze`, `write`, flags existentes, MBR como default e confirmação destrutiva existente.

```text
image2usb write <ISO> --mode auto|dd|extract
  --filesystem auto|fat32|ntfs         # default auto
  --bypass-tpm                       # default false, Windows 11
  --bypass-secure-boot               # default false, exigência do setup
  --offline-account                 # default false, builds comprovadas
  --secure-boot-help                 # ajuda sem gravação
```

- `--secure-boot-help` é uma ação exclusiva de consulta no comando write: pode omitir ISO/destino, retorna antes de analisar/abrir qualquer arquivo ou dispositivo; recusar combinação com flags de gravação/personalização. Ajustar ISO para obrigatório somente fora dessa ação. Explicar distinção setup/firmware, perda de proteção, chave BitLocker disponível antes de mudar firmware e instruções dependentes do fabricante; nunca executar comandos de firmware.
- `--filesystem auto` não altera default Linux FAT32 nem lógica DD existente. `--filesystem ntfs` é suportado para extração Windows neste marco; rejeitar Linux NTFS e opções Windows em DD ou imagens não Windows antes de abrir destino. Escolha explícita de filesystem em DD também deve ser recusada como incompatível, não ignorada.
- Auto Windows não híbrido escolhe extract; não anunciar DD inicializável só por haver El Torito. Preservar DD explicitamente solicitado sem aplicar personalizações.
- `--dry-run` permite preparação temporária e leitura completas para determinar viabilidade, mas nenhuma escrita no destino. Mostrar progresso/custo scratch; limpar temporários ao final.
- Plano final mostra identidade destino, modo, esquema, filesystem solicitado/resolvido, motivo, layout, split/arquivos preservados, ajustes individuais, verificação, scratch e avisos de Secure Boot.
- `--yes` dispensa prompt de apagamento, nunca dispensa validação ou proteção da ISO original. `--output` existente mantém confirmação; mesmo arquivo por hard/symlink é proibido mesmo com `--yes`.

## Analyze JSON

Manter campos existentes de IsoInfo e `windows.install_wim_size`. Novos campos Windows são aditivos e serializados em snake_case: arquitetura, formato, partes e capacidade de ajustes. Windows pode ser candidato com compatibilidade negativa/unknown; erros devem explicar causa. Não persistir caminhos temporários ou segredos no JSON.

## GUI

- Selector filesystem Automatic/FAT32/NTFS; três checkboxes de personalização independentes false. Ajuda Secure Boot é botão separado, sem efeito de checkbox ou escrita.
- Windows selecionado não desabilita gravação genericamente. Desabilitar por incompatibilidade específica, estado ocupado ou validação pendente.
- Preparação em worker mantém interface utilizável; mostrar Preparing e cancelamento. Plano final deve ser confirmado depois de seleção automática e preparação; mudança de opção/origem/destino invalida confirmação.
- Controles com labels acessíveis, operação por teclado e ajuda textual; seguir idioma e estilos existentes, sem iniciar projeto separado de localização.

## API interna compartilhada

- Preparação recebe origem read-only, opções, geometria/capacidade do destino, cancel token e progress sink. Captura snapshot privado integral, compara identidade/metadados antes/depois e aborta mudança detectada; análise definitiva/catálogo e leituras posteriores usam somente snapshot. Scratch inclui ISO inteira e transformações. Não recebe RawIo gravável nem abre destino.
- Retorna plano concreto + recursos preparados com ciclo de vida próprio; erro não deixa arquivo destino alterado.
- Commit recebe preparação confirmada e RawIo exclusivo; revalida geometria e recurso privado preparado, aplica layout, copia e verifica. Alteração posterior na ISO original não afeta o snapshot usado. Reutilizar RawIo, ProgressSink e erros existentes; tipos novos apenas quando necessários.
- `Preparing` é variante aditiva de Phase; atualizar matches, serialização e renderização nos consumidores.
- FFI native captura short I/O, bounds, errno e cancelamento; nenhuma exceção/panic cruza ABI, ponteiros não escapam o lifetime do wrapper. Formatter global não é reentrante.

## Erros e segurança

Categorias: origem inválida, arquitetura desconhecida, boot ausente, configuração incompatível, SWM inválido, partes grandes, scratch/destino insuficiente, recurso distribuído inválido, identidade alterada, cancelamento, I/O, verificação.

Mensagens incluem causa e ação possível; I/O de destino inclui offset quando conhecido. Reutilizar códigos de saída atuais e documentar novos mapeamentos antes de adicioná-los; não inventar códigos aqui. Erro/cancelamento após primeira escrita inclui aviso de mídia incompleta. Não fazer retry destrutivo nem downgrade silencioso de Secure Boot.

## Suporte Secure Boot

Sucesso declarado apenas para firmware/certificados/DBX testados. FAT32 mantém EFI Microsoft; NTFS usa asset upstream assinado intacto. Setup bypass não faz firmware aceitar binário revogado. Não detectar nem inferir configuração do computador alvo pela configuração do hospedeiro.
