/** 表单里的值和已保存的配置不一样吗。「测试连接」据此注明「测的是还没保存的值」。 */
export function isUnsavedForm(
  saved: { base_url: string; model: string },
  form: { baseUrl: string; model: string; key: string },
): boolean {
  return (
    form.baseUrl.trim().replace(/\/+$/, "") !== saved.base_url ||
    form.model.trim() !== saved.model ||
    form.key.trim() !== ""
  );
}
