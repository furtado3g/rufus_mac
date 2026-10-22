# Data model — Windows media

Estender tipos existentes; nenhum armazenamento persistente de trabalhos, framework ou camada repository.

## Opções e análise

- **FilesystemSelection**: Auto, Fat32, Ntfs; entrada CLI/GUI. `FileSystem` existente continua sendo apenas resultado concreto Fat32/Ntfs; não introduzir Auto no writer.
- **WindowsOptions**: bypass_tpm, bypass_secure_boot, offline_account; bools independentes, false por padrão. Substituir `bypass_win11` genérico não utilizado, atualizar serialização e todos os construtores internos; aceitar campos ausentes com defaults. Não manter flag genérica que ative tudo silenciosamente.
- **WindowsInfo** existente: arquitetura x64/arm64/unknown, versão/build/edição quando identificáveis, formato WIM/ESD/SWM, tamanhos u64, conjunto de partes, fallback PE validado, capacidades de ajustes por build. Marcadores indicam candidato, não validação de boot.
- **SourceEntry**: caminho relativo normalizado, tipo arquivo/diretório, tamanho u64, referências a extents ISO ou handle UDF; relações hierárquicas e ordem de diretórios antes de arquivos. Reutilizar representação existente onde possível; não guardar bytes completos.

Validações: caminhos sem absoluto/`..`/NUL/links; limites de entrada e aritmética checked; extents dentro da origem; ambiguidades case-insensitive em caminhos Windows críticos falham. ISO multi-extent precisa juntar sequência corretamente. PE machine coerente com arquitetura; unknown impede gravação Windows.

## Plano preparado

Estender **WritePlan** com filesystem solicitado/resolvido, motivo da seleção, WindowsOptions, requisitos scratch, partições e transformações. Resultado preparado possui workspace temporário e handle do snapshot privado integral; análise definitiva e leituras posteriores usam esse snapshot, não são reconstruídas por cada frontend. Scratch inclui ISO inteira e transformações; comparar identidade/metadados da origem antes/depois da captura e abortar mudança detectada. Snapshot estabiliza bytes usados, não comprova autenticidade ou detecção adversarial universal durante captura.

- **Partição**: papel dados/uefi_ntfs, start/length u64, setor lógico, tipo MBR ou GUID GPT, filesystem. Sem overlap, alinhada 1 MiB, dentro do dispositivo; proteção de headers GPT primário/backup e ESP auxiliar. Tamanho de asset derivado do arquivo real.
- **Arquivo preparado**: caminho destino, origem (catálogo ou scratch), tamanho, transformação (nenhuma, WIM split, fallback EFI, answer file), SHA-256 esperado quando verificação habilitada. SWM recebe part number/total/GUID coerentes. Não sobrescrever conjunto SWM original com novo split.
- **Recurso distribuído**: identificação, versão/commit, plataforma/arquitetura, tamanho/hash, licença, fonte correspondente e receita. Helpers do host distintos de boot assets da arquitetura alvo.

Auto prioriza FAT32 e divide WIM grande. ESD/outro arquivo grande ou WIM impossível de dividir validamente resolve NTFS; escolha manual incompatível falha antes de commit. Mudança de resolução invalida confirmação anterior.

## Estados

`Analyzing → Planning → Preparing → Ready → Confirmed → Opening → Formatting → Copying → Finalizing → Verifying (opcional) → Done`.

- Antes de Opening: destino não modificado; cancelamento/erro elimina somente recursos próprios.
- Mudança detectada da origem durante captura ou identidade do destino alterada: plano inválido, exigir nova análise/confirmação; nunca continuar automaticamente. Mudança posterior na origem não afeta o plano baseado no snapshot privado.
- Após primeira escrita: erro/cancelamento produz Failed/Cancelled com `target_incomplete = true`.
- Done exige flush; resultado registra verificação executada ou não e avisos de ejeção separadamente.
- Preparing tem progresso; chamadas indivisíveis são documentadas. Cancelamento de helper deve aguardar encerramento antes de limpar seus arquivos.

## Evidência

Arquivo documental por execução: commit, ISO hash/build/arquitetura, host, modo filesystem/esquema, opções, firmware/QEMU, Secure Boot on/off, DB/DBX/certificados, componentes e assinaturas, dispositivo autorizado quando físico, passos, tela/log e resultado. `Not tested` não se converte em `Pass`.
