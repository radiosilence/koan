{{/*
Chart name, truncated for use in resource names.
*/}}
{{- define "koan.name" -}}
{{- .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
Fully qualified app name, respecting nameOverride/fullnameOverride if ever
added, and collapsing to "koan" when the release is also named "koan".
*/}}
{{- define "koan.fullname" -}}
{{- if eq .Release.Name .Chart.Name -}}
{{- .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "koan.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "koan.labels" -}}
helm.sh/chart: {{ include "koan.chart" . }}
{{ include "koan.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{- define "koan.selectorLabels" -}}
app.kubernetes.io/name: {{ include "koan.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/*
The container image: image.tag if set, otherwise v{{ Chart.AppVersion }} --
the appVersion tracks the workspace version, which is what each release
publishes.
*/}}
{{- define "koan.image" -}}
{{- printf "%s:%s" .Values.image.repository (.Values.image.tag | default (printf "v%s" .Chart.AppVersion)) -}}
{{- end -}}

{{- define "koan.apiServiceName" -}}
{{- .Values.service.name | default (include "koan.fullname" .) -}}
{{- end -}}

{{- define "koan.mcpServiceName" -}}
{{- .Values.mcp.serviceName | default (printf "%s-mcp" (include "koan.fullname" .)) -}}
{{- end -}}
