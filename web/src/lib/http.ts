import axios, { AxiosInstance, AxiosRequestConfig } from 'axios';

import { getCsrfToken, removeToken } from '@/lib/cookie.ts';
import { getBaseUrl } from '@/lib/service.ts';

export type ApiResponse<T = any> = {
  code: number;
  msg: string;
  data: T;
};

class Http {
  private instance: AxiosInstance;

  constructor() {
    const baseURL = getBaseUrl('http');

    this.instance = axios.create({
      baseURL,
      withCredentials: true,
      timeout: 60 * 1000
    });

    this.setInterceptors();
  }

  private setInterceptors() {
    this.instance.interceptors.request.use((config) => {
      if (config.headers) {
        config.headers.Accept = 'application/json';
        const method = config.method?.toLowerCase();
        const csrfToken = getCsrfToken();
        if (csrfToken && method && ['post', 'put', 'patch', 'delete'].includes(method)) {
          config.headers['x-csrf-token'] = csrfToken;
        }
      }

      return config;
    });

    this.instance.interceptors.response.use(
      (response) => {
        return response.data;
      },
      (error) => {
        const code = error.response?.status;
        if (code === 401) {
          removeToken();
          window.location.reload();
        }
        const msg = error.response?.data?.msg;
        if (code === 403 && typeof msg === 'string' && msg.toLowerCase().includes('csrf')) {
          removeToken();
          window.location.reload();
        }
        return Promise.reject(error);
      }
    );
  }

  public get<T = any>(url: string, params?: any): Promise<ApiResponse<T>> {
    return this.instance.request({
      method: 'get',
      url,
      params
    });
  }

  public post<T = any>(
    url: string,
    data?: any,
    config?: AxiosRequestConfig
  ): Promise<ApiResponse<T>> {
    return this.instance.request({
      method: 'post',
      url,
      data,
      ...config
    });
  }

  public delete<T = any>(url: string, data?: any): Promise<ApiResponse<T>> {
    return this.instance.request({
      method: 'delete',
      url,
      data
    });
  }

  public request<T = any>(config: AxiosRequestConfig): Promise<ApiResponse<T>> {
    return this.instance.request(config);
  }
}

export const http = new Http();
